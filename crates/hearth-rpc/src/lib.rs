use hearth_types::*;

use remoc::rtc::{remote, CallError};

pub use remoc;
pub use hearth_types;

pub type CallResult<T> = Result<T, CallError>;

/// An interface for acquiring access to a client's remote APIs.
///
/// This is an example of the [Service Locator design pattern](https://gameprogrammingpatterns.com/service-locator.html).
/// This is considered an anti-pattern by some because services acquired
/// through it cannot be easily tested. However, this is not an issue in this
/// usecase because all this interface provides access to are procedural client
/// implementations to the real remote implementation, which could be made
/// testable with mocks at no consequence on this interface.
#[remote]
pub trait ClientApiProvider {
    async fn get_process_api(&self) -> CallResult<ProcessApiClient>;
}

#[remote]
pub trait ProcessApi {
    async fn print_hello_world(&self) -> CallResult<()>;
    async fn spawn(&self, module: AssetId, peer: PeerId, linked: bool) -> CallResult<ProcessId>;
    async fn link(&self, pid: ProcessId) -> CallResult<()>; // TODO watcher channel for child errors?
    async fn unlink(&self, pid: ProcessId) -> CallResult<()>;
    async fn kill(&self, pid: ProcessId) -> CallResult<()>;
    // TODO Lunatic Supervisor-like child API?
}
