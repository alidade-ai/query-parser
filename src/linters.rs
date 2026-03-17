mod no_wildcard;

use crate::parser::LinterPipeline;

pub use no_wildcard::NoWildcard;

pub fn default_pipeline() -> LinterPipeline {
    let mut pipeline = LinterPipeline::new();
    pipeline.add(NoWildcard);
    pipeline
}
