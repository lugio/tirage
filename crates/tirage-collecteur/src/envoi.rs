//! Envoi de la file d'attente au serveur.
//!
//! Un lot part, le serveur répond élément par élément : les acquittés sont retirés de la file, les refusés mis de
//! côté (dossier `rebut`), les autres restent pour le prochain essai. Serveur injoignable = on attend et on réessaie,
//! avec un délai croissant ; rien n'est perdu.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tirage_file::{ErreurFile, FileAttente};

use crate::config::Serveur;

/// Événements par lot (le serveur en accepte au plus 500).
pub const LOT: usize = 200;

/// Poste un corps JSON, rend (code HTTP, texte de la réponse).
pub trait Expediteur {
    fn poster(&mut self, corps: &[u8]) -> Result<(u16, String), String>;
}

#[derive(Debug, Deserialize, PartialEq)]
struct Reponse {
    acquittes: Vec<u64>,
    refuses: Vec<Refus>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Refus {
    id: u64,
    raison: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Bilan {
    Vide,
    Envoye { acquittes: usize, refuses: usize },
}

#[derive(Debug)]
pub enum ErreurEnvoi {
    /// Serveur injoignable ou indisponible (réseau, 5xx) : réessayer plus tard.
    Passagere(String),
    /// Le serveur refuse le collecteur (jeton, configuration) : réessayer, mais le signaler.
    Refus(u16, String),
    Disque(ErreurFile),
}

impl std::fmt::Display for ErreurEnvoi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErreurEnvoi::Passagere(e) => write!(f, "serveur injoignable : {e}"),
            ErreurEnvoi::Refus(c, e) => write!(f, "serveur : refus {c} ({e})"),
            ErreurEnvoi::Disque(e) => write!(f, "{e}"),
        }
    }
}

impl From<ErreurFile> for ErreurEnvoi {
    fn from(e: ErreurFile) -> Self {
        ErreurEnvoi::Disque(e)
    }
}

/// Envoie un lot ; rend ce qui a été fait.
pub fn envoyer_un_lot<E: Expediteur>(
    file: &mut FileAttente,
    exp: &mut E,
    max: usize,
) -> Result<Bilan, ErreurEnvoi> {
    let lot = file.lot(max)?;
    if lot.is_empty() {
        return Ok(Bilan::Vide);
    }
    let mut corps = Vec::with_capacity(lot.iter().map(|e| e.octets.len() + 32).sum());
    corps.extend_from_slice(b"{\"evenements\":[");
    let mut premier = true;
    let mut mis_de_cote = 0;
    for e in &lot {
        // Un élément qui n'est pas du JSON casserait tout le lot : il est mis de côté ici.
        if serde_json::from_slice::<&serde_json::value::RawValue>(&e.octets).is_err() {
            file.mettre_de_cote(e.id)?;
            mis_de_cote += 1;
            continue;
        }
        if !premier {
            corps.push(b',');
        }
        premier = false;
        corps.extend_from_slice(format!("{{\"id\":{},\"enveloppe\":", e.id).as_bytes());
        corps.extend_from_slice(&e.octets);
        corps.push(b'}');
    }
    corps.extend_from_slice(b"]}");
    if premier {
        return Ok(Bilan::Envoye {
            acquittes: 0,
            refuses: mis_de_cote,
        });
    }
    let (code, texte) = exp.poster(&corps).map_err(ErreurEnvoi::Passagere)?;
    match code {
        200 => {}
        500..=599 | 408 | 429 => return Err(ErreurEnvoi::Passagere(format!("HTTP {code}"))),
        _ => return Err(ErreurEnvoi::Refus(code, texte.chars().take(200).collect())),
    }
    let r: Reponse = serde_json::from_str(&texte)
        .map_err(|e| ErreurEnvoi::Passagere(format!("réponse illisible : {e}")))?;
    for id in &r.acquittes {
        file.acquitter(*id)?;
    }
    for refus in &r.refuses {
        eprintln!("événement {} refusé par le serveur : {}", refus.id, refus.raison);
        file.mettre_de_cote(refus.id)?;
    }
    Ok(Bilan::Envoye {
        acquittes: r.acquittes.len(),
        refuses: r.refuses.len() + mis_de_cote,
    })
}

/// Délais d'attente de la boucle.
#[derive(Debug, Clone, Copy)]
pub struct Rythme {
    pub file_vide: Duration,
    pub erreur_min: Duration,
    pub erreur_max: Duration,
}

impl Default for Rythme {
    fn default() -> Self {
        Rythme {
            file_vide: Duration::from_secs(5),
            erreur_min: Duration::from_secs(5),
            erreur_max: Duration::from_secs(300),
        }
    }
}

fn patienter(d: Duration, arret: &AtomicBool) {
    let fin = Instant::now() + d;
    while Instant::now() < fin && !arret.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100).min(fin.saturating_duration_since(Instant::now())));
    }
}

/// Vide la file en continu jusqu'à `arret`. `journal` reçoit les changements d'état (pas un message par lot).
pub fn boucle<E: Expediteur>(
    donnees: &Path,
    capacite: usize,
    exp: &mut E,
    rythme: Rythme,
    arret: &AtomicBool,
    mut journal: impl FnMut(&str),
) -> Result<(), ErreurFile> {
    let mut file = FileAttente::ouvrir(donnees, capacite)?;
    let mut attente = rythme.erreur_min;
    let mut en_erreur = false;
    while !arret.load(Ordering::Relaxed) {
        match envoyer_un_lot(&mut file, exp, LOT) {
            Ok(Bilan::Vide) => {
                if en_erreur {
                    journal("envoi rétabli");
                    en_erreur = false;
                }
                attente = rythme.erreur_min;
                patienter(rythme.file_vide, arret);
            }
            Ok(Bilan::Envoye { acquittes, refuses }) => {
                if en_erreur {
                    journal("envoi rétabli");
                    en_erreur = false;
                }
                attente = rythme.erreur_min;
                if refuses > 0 {
                    journal(&format!("{acquittes} envoyé(s), {refuses} mis de côté"));
                }
            }
            Err(ErreurEnvoi::Disque(e)) => return Err(e),
            Err(e) => {
                if !en_erreur {
                    journal(&format!("{e} — nouvel essai dans {} s", attente.as_secs()));
                    en_erreur = true;
                }
                patienter(attente, arret);
                attente = (attente * 2).min(rythme.erreur_max);
            }
        }
    }
    Ok(())
}

/// Expéditeur HTTP(S) réel.
pub struct Http {
    agent: ureq::Agent,
    url: String,
    autorisation: String,
}

impl Http {
    pub fn nouveau(s: &Serveur) -> Result<Self, String> {
        use ureq::tls::{Certificate, RootCerts, TlsConfig};
        let url = format!("{}/api/v1/evenements", s.url.trim_end_matches('/'));
        if url.starts_with("http://") && !s.http_en_clair {
            return Err("URL en http:// refusée : utiliser https://, ou poser `http_en_clair = true` (essai seulement)".into());
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(format!("URL du serveur invalide : {}", s.url));
        }
        let racines = match &s.ca {
            Some(chemin) => {
                let pem = std::fs::read(chemin).map_err(|e| format!("{} : {e}", chemin.display()))?;
                let cert = Certificate::from_pem(&pem).map_err(|e| format!("{} : {e}", chemin.display()))?;
                RootCerts::new_with_certs(&[cert])
            }
            None => RootCerts::PlatformVerifier,
        };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .tls_config(TlsConfig::builder().root_certs(racines).build())
            .build()
            .into();
        Ok(Http {
            agent,
            url,
            autorisation: format!("Bearer {}", s.jeton),
        })
    }
}

impl Expediteur for Http {
    fn poster(&mut self, corps: &[u8]) -> Result<(u16, String), String> {
        let mut rep = self
            .agent
            .post(&self.url)
            .header("Authorization", &self.autorisation)
            .content_type("application/json")
            .send(corps)
            .map_err(|e| e.to_string())?;
        let code = rep.status().as_u16();
        let texte = rep.body_mut().read_to_string().map_err(|e| e.to_string())?;
        Ok((code, texte))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Faux serveur : acquitte tout sauf les ids listés comme refusés ; ou répond un code d'erreur.
    struct Faux {
        code: u16,
        refuser: Vec<u64>,
        recus: Vec<Vec<u64>>,
    }

    impl Expediteur for Faux {
        fn poster(&mut self, corps: &[u8]) -> Result<(u16, String), String> {
            if self.code == 0 {
                return Err("connexion refusée".into());
            }
            let v: serde_json::Value = serde_json::from_slice(corps).unwrap();
            let ids: Vec<u64> = v["evenements"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["id"].as_u64().unwrap())
                .collect();
            self.recus.push(ids.clone());
            let (ref_, acq): (Vec<u64>, Vec<u64>) = ids.into_iter().partition(|i| self.refuser.contains(i));
            let refuses: Vec<String> = ref_
                .iter()
                .map(|i| format!("{{\"id\":{i},\"raison\":\"essai\"}}"))
                .collect();
            Ok((
                self.code,
                format!("{{\"acquittes\":{acq:?},\"refuses\":[{}]}}", refuses.join(",")),
            ))
        }
    }

    fn faux(code: u16) -> Faux {
        Faux {
            code,
            refuser: vec![],
            recus: vec![],
        }
    }

    #[test]
    fn lot_acquitte_et_refus_mis_de_cote() {
        let d = tempfile::tempdir().unwrap();
        let mut f = FileAttente::ouvrir(d.path(), 100).unwrap();
        for i in 0..3 {
            f.deposer(format!("{{\"n\":{i}}}").as_bytes()).unwrap();
        }
        f.deposer(b"pas du json").unwrap();
        let mut s = Faux {
            refuser: vec![2],
            ..faux(200)
        };
        assert_eq!(
            envoyer_un_lot(&mut f, &mut s, 10).unwrap(),
            Bilan::Envoye {
                acquittes: 2,
                refuses: 2
            }
        );
        assert_eq!(
            s.recus,
            vec![vec![1, 2, 3]],
            "l'élément illisible n'est pas parti"
        );
        assert!(f.est_vide() || f.lot(10).unwrap().is_empty());
        assert!(d.path().join("rebut").join(format!("{:020}.evt", 2)).exists());
        assert!(d.path().join("rebut").join(format!("{:020}.evt", 4)).exists());
        assert_eq!(envoyer_un_lot(&mut f, &mut s, 10).unwrap(), Bilan::Vide);
    }

    #[test]
    fn serveur_injoignable_rien_ne_se_perd() {
        let d = tempfile::tempdir().unwrap();
        let mut f = FileAttente::ouvrir(d.path(), 100).unwrap();
        f.deposer(b"{}").unwrap();
        assert!(matches!(
            envoyer_un_lot(&mut f, &mut faux(0), 10),
            Err(ErreurEnvoi::Passagere(_))
        ));
        assert!(matches!(
            envoyer_un_lot(&mut f, &mut faux(503), 10),
            Err(ErreurEnvoi::Passagere(_))
        ));
        assert!(matches!(
            envoyer_un_lot(&mut f, &mut faux(401), 10),
            Err(ErreurEnvoi::Refus(401, _))
        ));
        assert_eq!(f.lot(10).unwrap().len(), 1);
    }

    #[test]
    fn reponse_partielle_garde_le_reste() {
        struct Moitie;
        impl Expediteur for Moitie {
            fn poster(&mut self, _: &[u8]) -> Result<(u16, String), String> {
                Ok((200, r#"{"acquittes":[1],"refuses":[]}"#.into()))
            }
        }
        let d = tempfile::tempdir().unwrap();
        let mut f = FileAttente::ouvrir(d.path(), 100).unwrap();
        f.deposer(b"{}").unwrap();
        f.deposer(b"{}").unwrap();
        envoyer_un_lot(&mut f, &mut Moitie, 10).unwrap();
        assert_eq!(
            f.lot(10).unwrap().iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn boucle_s_arrete_et_vide_la_file() {
        let d = tempfile::tempdir().unwrap();
        {
            let mut f = FileAttente::ouvrir(d.path(), 1000).unwrap();
            for _ in 0..450 {
                f.deposer(b"{}").unwrap();
            }
        }
        let arret = AtomicBool::new(false);
        let mut s = faux(200);
        let rythme = Rythme {
            file_vide: Duration::from_millis(10),
            erreur_min: Duration::from_millis(10),
            erreur_max: Duration::from_millis(20),
        };
        std::thread::scope(|sc| {
            sc.spawn(|| {
                std::thread::sleep(Duration::from_millis(300));
                arret.store(true, Ordering::Relaxed);
            });
            boucle(d.path(), 1000, &mut s, rythme, &arret, |_| {}).unwrap();
        });
        assert_eq!(
            s.recus.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![200, 200, 50]
        );
        assert!(FileAttente::ouvrir(d.path(), 10).unwrap().est_vide());
    }

    #[test]
    fn http_en_clair_refuse_par_defaut() {
        let s = Serveur {
            url: "http://192.0.2.1:8080".into(),
            jeton: "x".into(),
            ca: None,
            http_en_clair: false,
        };
        assert!(Http::nouveau(&s).is_err());
        assert!(
            Http::nouveau(&Serveur {
                http_en_clair: true,
                ..s.clone()
            })
            .is_ok()
        );
        assert!(
            Http::nouveau(&Serveur {
                url: "ftp://x".into(),
                ..s
            })
            .is_err()
        );
    }
}
