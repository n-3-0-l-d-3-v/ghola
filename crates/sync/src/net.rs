//! A `Remote` that talks to a repository through distrans: an RPC client and
//! server over a reliable transport connection over a hostile simulated
//! channel (loss, duplication, reordering, corruption, truncation), in
//! virtual time and reproducible from a seed. Requests are issued one at a
//! time; each `call` advances the simulation until its response arrives.

use channel::{Channel, FaultProfile, FaultStats, Tick};
use repo::Repo;
use rpc::{Handler, RpcClient, RpcServer};
use transport::{Config as TransportConfig, Connection};

use crate::client::{Remote, SyncError};
use crate::server::serve;

#[derive(Debug, Clone)]
pub struct NetConfig {
    pub seed: u64,
    pub profile: FaultProfile,
    /// Base ticks before the RPC client resends an unanswered request (grows
    /// with the request's size, see `call`).
    pub retry_deadline: u64,
    pub max_attempts: u32,
    /// Give up on one call after this many ticks.
    pub max_ticks_per_call: u64,
}

impl NetConfig {
    pub fn clean(seed: u64) -> Self {
        NetConfig {
            seed,
            profile: FaultProfile::CLEAN,
            retry_deadline: 2_000,
            max_attempts: 40,
            max_ticks_per_call: 400_000,
        }
    }

    pub fn hostile(seed: u64, loss: f64) -> Self {
        NetConfig {
            profile: FaultProfile {
                loss,
                duplication: 0.1,
                reorder_max_delay: 12,
                corruption: 0.03,
                truncation: 0.03,
                base_delay: 2,
            },
            ..NetConfig::clean(seed)
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NetStats {
    pub ticks: u64,
    pub calls: u64,
    pub client_to_server: FaultStats,
    pub server_to_client: FaultStats,
}

pub struct DistransRemote<'a> {
    client: RpcClient,
    server: RpcServer<'a>,
    c2s: Channel,
    s2c: Channel,
    now: u64,
    calls: u64,
    cfg: NetConfig,
}

impl<'a> DistransRemote<'a> {
    /// Serves `server_repo` over a simulated hostile link for as long as
    /// the returned remote lives.
    pub fn new(server_repo: &'a mut Repo, cfg: NetConfig) -> Self {
        let tcfg = TransportConfig {
            max_retries: 400,
            ..TransportConfig::default()
        };
        let (client_conn, syn) = Connection::connect(tcfg, Tick(0));
        let server_conn = Connection::listen(tcfg, Tick(0));
        let handler: Handler<'a> =
            Box::new(move |method, payload| serve(server_repo, method, payload));
        let mut c2s = Channel::new(cfg.seed, cfg.profile);
        let s2c = Channel::new(cfg.seed ^ 0xABCD_EF01_2345_6789, cfg.profile);
        c2s.send(&syn);
        DistransRemote {
            client: RpcClient::new(client_conn, cfg.retry_deadline, cfg.max_attempts),
            server: RpcServer::new(server_conn, handler),
            c2s,
            s2c,
            now: 0,
            calls: 0,
            cfg,
        }
    }

    pub fn stats(&self) -> NetStats {
        NetStats {
            ticks: self.now,
            calls: self.calls,
            client_to_server: self.c2s.stats(),
            server_to_client: self.s2c.stats(),
        }
    }

    /// How many times the server-side handler actually ran (each distinct
    /// request exactly once, however often it was retried).
    pub fn executions(&self) -> u64 {
        self.server.executions()
    }
}

impl Remote for DistransRemote<'_> {
    fn call(&mut self, method: u8, payload: &[u8]) -> Result<(bool, Vec<u8>), SyncError> {
        // The RPC layer resends the *whole* request every `retry_deadline`
        // ticks, on top of the transport's own reliable retransmission. With a
        // fixed short deadline a big request is re-queued faster than the
        // link can drain it and the connection collapses under its own
        // retries (found by the 300 KB-object test), so the deadline grows
        // with the payload. Loss is already handled by the transport.
        self.client.retry_deadline = self.cfg.retry_deadline + 4 * payload.len() as u64;
        let (id, out) = self.client.call(Tick(self.now), method, payload);
        self.calls += 1;
        for d in out {
            self.c2s.send(&d);
        }
        let start = self.now;
        loop {
            self.now += 1;
            let now = Tick(self.now);
            for d in self.c2s.advance(now) {
                for o in self.server.on_datagram(now, &d) {
                    self.s2c.send(&o);
                }
            }
            for d in self.s2c.advance(now) {
                for o in self.client.on_datagram(now, &d) {
                    self.c2s.send(&o);
                }
            }
            for o in self.client.on_tick(now) {
                self.c2s.send(&o);
            }
            for o in self.server.on_tick(now) {
                self.s2c.send(&o);
            }
            for (rid, resp) in self.client.poll_completed() {
                if rid == id {
                    return Ok((resp.ok, resp.payload));
                }
            }
            if self.client.poll_failed().contains(&id) {
                return Err(SyncError::Transport(format!(
                    "request gave up after {} attempts",
                    self.cfg.max_attempts
                )));
            }
            if self.now - start > self.cfg.max_ticks_per_call {
                return Err(SyncError::Transport(format!(
                    "no response within {} ticks",
                    self.cfg.max_ticks_per_call
                )));
            }
        }
    }
}
