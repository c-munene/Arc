use crate::config::RhaiScriptConfig;
use ahash::AHashMap;
use rhai::{Engine, Scope, AST};
use std::sync::Arc;

/// A compiled Rhai script.
#[derive(Clone)]
pub struct RhaiScript {
    pub name: Arc<str>,
    pub ast: Arc<AST>,
    pub max_ops: u64,
}

/// Registry of Rhai scripts.
#[derive(Clone)]
pub struct RhaiRegistry {
    scripts: Arc<AHashMap<Arc<str>, RhaiScript>>,
}

impl RhaiRegistry {
    pub fn build(cfgs: &[RhaiScriptConfig]) -> anyhow::Result<Self> {
        let mut engine = Engine::new();
        // No file IO, no module resolver by default.
        engine.set_max_call_levels(64);

        let mut scripts = AHashMap::new();
        for s in cfgs {
            let ast = engine.compile(&s.inline)?;
            scripts.insert(
                Arc::<str>::from(s.name.clone()),
                RhaiScript {
                    name: Arc::<str>::from(s.name.clone()),
                    ast: Arc::new(ast),
                    max_ops: s.max_ops,
                },
            );
        }

        Ok(Self {
            scripts: Arc::new(scripts),
        })
    }

    pub fn get(&self, name: &str) -> Option<RhaiScript> {
        self.scripts.get(name).cloned()
    }

    /// Execute a script entry function with a structured context.
    ///
    /// For production, provide a typed API (ctx.headers.get, ctx.headers.set, etc).
    pub fn run(&self, script: &RhaiScript, mut scope: Scope) -> anyhow::Result<()> {
        let mut engine = Engine::new();
        engine.set_max_call_levels(64);
        engine.set_max_operations(script.max_ops);

        // Convention: optional `on_request(ctx)`.
        let _ = engine
            .call_fn::<()>(&mut scope, &script.ast, "on_request", ())
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}
