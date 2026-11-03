//! Collecteur : suivi des travaux (spouleur Windows) et relevé des copieurs (SNMP).

pub mod config;
pub mod copieurs;
pub mod envoi;
pub mod execution;
pub mod journal;
#[cfg(windows)]
pub mod service;
pub mod spouleur;
