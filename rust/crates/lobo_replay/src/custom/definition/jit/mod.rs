//! Server-side Cranelift compilation of protocol decoders and operations.
//! Generated functions accept bytes and pass typed operands to concrete callbacks.
//! Compiler dependencies and executable memory are excluded from WASM builds.
pub(super) mod binary;
mod compiler;
mod typed;
pub(super) use typed::program::{Program, prepare as prepare_program};
pub(crate) use typed::runtime::ExecutionState;

use cranelift_jit::JITModule;
use std::{
    ops::{Deref, DerefMut},
    sync::Mutex,
};

type Entry = unsafe extern "C" fn(*mut std::ffi::c_void);
// Cranelift intentionally leaks code on ordinary JITModule::drop. Own that
// lifecycle explicitly, including errors before finalization.
struct ModuleMemory(Option<JITModule>);
impl ModuleMemory {
    fn new(module: JITModule) -> Self {
        Self(Some(module))
    }
}
impl Deref for ModuleMemory {
    type Target = JITModule;
    fn deref(&self) -> &JITModule {
        self.0.as_ref().expect("module is owned until drop")
    }
}
impl DerefMut for ModuleMemory {
    fn deref_mut(&mut self) -> &mut JITModule {
        self.0.as_mut().expect("module is owned until drop")
    }
}
impl Drop for ModuleMemory {
    fn drop(&mut self) {
        if let Some(module) = self.0.take() {
            // SAFETY: during compilation no entry has escaped. After
            // finalization all entries retain the owning Executable in an Arc.
            unsafe {
                module.free_memory();
            }
        }
    }
}
struct Executable {
    entries: Vec<Entry>,
    // The finalized module is never accessed by executing threads. A mutex
    // gives ownership its Send/Sync requirements without an unsafe blanket impl.
    _module: Mutex<ModuleMemory>,
}

/// Prepare the complete default packet program before a Python adapter starts.
#[cfg(feature = "python")]
pub(crate) fn prepare_protocol(
    definition: std::sync::Arc<super::schema::Definition>,
    mode: lobo_context::FeedMode,
) -> Result<std::sync::Arc<super::schema::Definition>, String> {
    let program = prepare_program::<crate::custom::observer::Discard>(definition, mode)?;
    Ok(program.definition.clone())
}
