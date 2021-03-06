//! A runtime implementation that runs everything on the current thread.
use futures::future::{self, Future, FutureExt};

mod arbiter;
mod builder;
mod runtime;
mod system;

pub use self::arbiter::Arbiter;
pub use self::builder::{Builder, SystemRunner};
pub use self::runtime::Runtime;
pub use self::system::System;

#[cfg(not(test))] // Work around for rust-lang/rust#62127
pub use ntex_rt_macros::{rt_main as main, rt_test as test};

#[doc(hidden)]
pub use actix_threadpool as blocking;

/// Spawn a future on the current thread. This does not create a new Arbiter
/// or Arbiter address, it is simply a helper for spawning futures on the current
/// thread.
///
/// # Panics
///
/// This function panics if ntex system is not running.
#[inline]
pub fn spawn<F>(f: F) -> tokio::task::JoinHandle<F::Output>
where
    F: futures::Future + 'static,
{
    tokio::task::spawn_local(f)
}

/// Executes a future on the current thread. This does not create a new Arbiter
/// or Arbiter address, it is simply a helper for executing futures on the current
/// thread.
///
/// # Panics
///
/// This function panics if ntex system is not running.
#[inline]
pub fn spawn_fn<F, R>(f: F) -> tokio::task::JoinHandle<R::Output>
where
    F: FnOnce() -> R + 'static,
    R: Future + 'static,
{
    tokio::task::spawn_local(future::lazy(|_| f()).flatten())
}

/// Asynchronous signal handling
pub mod signal {
    #[cfg(unix)]
    pub mod unix {
        pub use tokio::signal::unix::*;
    }
    pub use tokio::signal::ctrl_c;
}

/// TCP/UDP/Unix bindings
pub mod net {
    pub use tokio::net::UdpSocket;
    pub use tokio::net::{TcpListener, TcpStream};

    #[cfg(unix)]
    pub mod unix {
        pub use tokio::net::{UnixDatagram, UnixListener, UnixStream};
    }

    #[cfg(unix)]
    pub use self::unix::*;
}

/// Utilities for tracking time.
pub mod time {
    pub use tokio::time::Instant;
    pub use tokio::time::{delay_for, delay_until, Delay};
    pub use tokio::time::{interval, interval_at, Interval};
    pub use tokio::time::{timeout, Timeout};
}
