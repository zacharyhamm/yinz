//! Explicit host callback registration for the installed-test interpreter.
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::Value;
use steel::{
    rvals::{Custom, IntoSteelVal, SteelVal},
    steel_vm::{engine::Engine, register_fn::RegisterFn},
};

type Callback = dyn Fn(&mut [SteelVal]) -> steel::rvals::Result<SteelVal> + Send + Sync;

#[derive(Clone)]
pub struct JsonCallback(Arc<Callback>);
impl Custom for JsonCallback {}

impl JsonCallback {
    pub fn call(&self, argument: Value) -> Result<Value> {
        let result = (self.0)(&mut [crate::to_scheme(&argument)?])
            .map_err(|error| anyhow!("calling mock reply: {error}"))?;
        if matches!(result, SteelVal::Void) {
            Ok(Value::Null)
        } else {
            crate::from_scheme(&result)
        }
    }
}

pub(super) fn register(
    engine: &mut Engine,
    receive: impl Fn(String, JsonCallback) -> Result<()> + Send + Sync + 'static,
) {
    engine.register_value(
        "%yinz-capture-callback",
        SteelVal::BuiltIn(|ctx, args| {
            Some((|| {
                if args.len() != 1 || !args[0].is_function() {
                    return Err(crate::scheme_error("callback requires one procedure"));
                }
                JsonCallback(ctx.steel_function_to_rust_function(args[0].clone()).into())
                    .into_steelval()
            })())
        }),
    );
    engine.register_fn(
        "%yinz-register-callback",
        move |name: String, callback: JsonCallback| -> steel::rvals::Result<()> {
            receive(name, callback).map_err(crate::scheme_error)
        },
    );
}
