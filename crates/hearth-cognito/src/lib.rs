use hearth_rpc::ProcessApi;
use hearth_wasm::{GuestMemory, WasmLinker};
use wasmtime::{Caller, Linker};

/// This contains all script-accessible process-related stuff.
pub struct Cognito {
    pub api: Box<dyn ProcessApi + Send + Sync>,
}

// Should automatically generate link_print_hello_world:
// #[impl_wasm_linker]
// should work for any struct, not just Cognito
impl Cognito {
    pub async fn print_hello_world(&self) {
        self.api.print_hello_world().await.unwrap();
    }

    // impl_wasm_linker should also work with non-async functions
    //
    // if a function is passed GuestMemory or GuestMemory<'_>, the macro should
    // automatically create a GuestMemory instance using the Caller's exported
    // memory extern
    //
    // it should also turn arguments in the core wasm types (u32, u64, i32, u64)
    // into arguments for the linker's closure, as well as the return type,
    // which in this example is just ().
    pub fn log_message(&self, mut memory: GuestMemory<'_>, msg_ptr: u32, msg_len: u32) {
        eprintln!("message from wasm: {}", memory.get_str(msg_ptr, msg_len));
    }

    // this is only generated; written up by hand for reference
    // remember to use absolute identifiers (prefixed with ::) for all references to structs
    pub fn link_print_hello_world<T: AsRef<Self> + Send>(linker: &mut Linker<T>) {
        async fn print_hello_world<T: AsRef<Cognito> + Send>(caller: Caller<'_, T>) {
            let cognito = caller.data().as_ref();
            cognito.print_hello_world().await;
        }

        linker
            // arity for the function name should be read by the proc macro
            // the module name can be derived by converting the struct's name to snake case
            .func_wrap0_async("cognito", "print_hello_world", |caller: Caller<'_, T>| {
                Box::new(print_hello_world(caller))
            })
            .unwrap();
    }

    pub fn link_log_message<T: AsRef<Self>>(linker: &mut Linker<T>) {
        linker
            .func_wrap(
                "cognito",
                "log_message",
                |mut caller: Caller<'_, T>, msg_ptr: u32, msg_len: u32| {
                    let memory = GuestMemory::from_caller(&mut caller);
                    // note that cognito needs to be retrieved after memory
                    // because it immutably borrows caller and memory does not
                    let cognito = caller.data().as_ref();
                    cognito.log_message(memory, msg_ptr, msg_len)
                },
            )
            .unwrap();
    }
}

// this impl block should also be generated by #[impl_wasm_linker] with all of
// the functions in its body
impl<T: AsRef<Cognito> + Send + 'static> WasmLinker<T> for Cognito {
    const MODULE_NAME: &'static str = "cognito";

    fn add_to_linker(linker: &mut Linker<T>) {
        Self::link_print_hello_world(linker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use hearth_rpc::{remoc, CallResult};
    use remoc::rtc::async_trait;

    struct MockProcessApi;

    #[async_trait]
    impl ProcessApi for MockProcessApi {
        async fn print_hello_world(&self) -> CallResult<()> {
            println!("Hello, world!");
            Ok(())
        }
    }

    #[test]
    fn host_works() {
        let api = Box::new(MockProcessApi);
        let cognito = Cognito { api };
        cognito.print_hello_world();
    }
}
