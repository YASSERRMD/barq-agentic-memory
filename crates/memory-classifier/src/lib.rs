//! Memory classification and extraction.
//!
//! The engine must function with zero LLM dependency: callers may
//! supply structured memories directly, or use the rule-based
//! classifier shipped here. External LLM / local-model / HTTP extractors
//! plug in behind the same traits.

pub mod classification;
pub mod extraction;
pub mod rules;

pub use classification::{Classification, ClassifierInput, MemoryClassifier};
pub use extraction::{ExtractedMemory, ExtractionProvider};
pub use rules::{RuleBasedClassifier, RuleBasedExtractor};
