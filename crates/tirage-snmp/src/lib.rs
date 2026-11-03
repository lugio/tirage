//! Client SNMP v2c minimal, en lecture seule, sans dépendance.
//!
//! Juste ce qu'il faut pour relever un copieur : GET, GETNEXT et parcours d'un sous-arbre. Aucune écriture (SET)
//! n'est possible : le type de PDU n'existe pas dans ce code. SNMP v3 viendra si un appareil l'impose.

pub mod ber;
pub mod client;

pub use ber::{Oid, Valeur};
pub use client::{Client, ErreurSnmp, Transport, Udp};
