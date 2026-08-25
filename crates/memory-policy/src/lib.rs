//! Governance hooks: authorization, policy, encryption, audit, and
//! data classification.
//!
//! The invariant this crate exists for: unauthorized memory must never
//! reach the calling model. Hooks are provider interfaces so deployments
//! plug in their own backends without engine changes.

pub mod audit;
pub mod authz;
pub mod crypto;
pub mod sensitivity;

pub use audit::{AuditEvent, Auditor};
pub use authz::{Authorizer, Principal};
pub use crypto::{AesGcmEncryptor, Encryptor, NoopEncryptor};
pub use sensitivity::{DataClassifier, Sensitivity, SensitivityClassifier};
