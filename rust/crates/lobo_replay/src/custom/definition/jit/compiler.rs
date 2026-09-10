use super::{Executable, ModuleMemory};
use cranelift_codegen::ir::{AbiParam, types};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use std::{collections::BTreeMap, sync::Mutex};

pub(super) struct Compiler {
    pub(super) module: ModuleMemory,
    pub(super) imports: BTreeMap<String, FuncId>,
    pub(super) functions: Vec<FuncId>,
}
impl Compiler {
    pub(super) fn with_symbols(symbols: &[(&str, *const u8, usize)]) -> Result<Self, String> {
        let mut builder = JITBuilder::with_flags(
            &[("opt_level", "speed")],
            cranelift_module::default_libcall_names(),
        )
        .map_err(|e| e.to_string())?;
        for &(name, address, _) in symbols {
            builder.symbol(name, address);
        }
        let mut module = ModuleMemory::new(JITModule::new(builder));
        if module.target_config().pointer_type() != types::I64 {
            return Err("The adapter JIT requires a 64-bit server target".into());
        }
        let mut imports = BTreeMap::new();
        for &(name, _, arity) in symbols {
            let mut signature = module.make_signature();
            signature.params = vec![AbiParam::new(types::I64); arity];
            signature.returns.push(AbiParam::new(types::I64));
            imports.insert(
                name.to_owned(),
                module
                    .declare_function(name, Linkage::Import, &signature)
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(Self {
            module,
            imports,
            functions: Vec::new(),
        })
    }
    pub fn finish(mut self) -> Result<Executable, String> {
        self.module
            .finalize_definitions()
            .map_err(|e| e.to_string())?;
        let entries = self
            .functions
            .iter()
            .map(|&id| {
                let address = self.module.get_finalized_function(id);
                // SAFETY: every declared entry has exactly this C calling
                // convention and signature. Executable retains its code memory.
                unsafe { std::mem::transmute::<*const u8, super::Entry>(address) }
            })
            .collect();
        Ok(Executable {
            entries,
            _module: Mutex::new(self.module),
        })
    }
}
#[derive(Clone, Copy)]
pub(super) enum Target {
    Time,
    Id,
    NewId,
    Side,
    Price,
    OptionalPrice,
    Quantity,
    TradeId,
    HistoryId,
    LevelPrice,
    LevelQuantity,
    Trader,
    Hidden,
    Peak,
}
