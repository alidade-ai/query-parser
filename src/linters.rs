mod bare_operator;
mod no_wildcard;

use crate::parser::LinterPipeline;

pub use bare_operator::BareOperator;
pub use no_wildcard::NoWildcard;

pub fn default_pipeline() -> LinterPipeline {
    let mut pipeline = LinterPipeline::new();
    pipeline.add(NoWildcard);
    pipeline.add(BareOperator);
    pipeline
}
