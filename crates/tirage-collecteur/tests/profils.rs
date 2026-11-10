//! Chaque profil de `profils/` est prouvé par un relevé d'essai (`profils/essais/<nom>.toml`).
//!
//! Le relevé d'essai reprend la sortie de `tirage-collecteur --parcourir` (ou de `snmpwalk -On`), réduite aux
//! nombres et aux OID : une ligne de texte (nom, emplacement, numéro de série…) ou une adresse est refusée, pour qu'un
//! relevé contribué ne publie rien sur l'appareil. Le test vérifie que le profil est choisi tout seul d'après
//! `sysObjectID`, que les compteurs calculés sont ceux attendus, et que noir + couleur redonne le total.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tirage_collecteur::copieurs::profil::INTEGRES;
use tirage_collecteur::copieurs::{Profils, relever};
use tirage_snmp::client::essai::FauxAgent;
use tirage_snmp::{Client, Oid, Valeur};

const SYS_OBJECT_ID: &str = "1.3.6.1.2.1.1.2.0";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Essai {
    /// Nom du profil que l'appareil doit recevoir.
    profil: String,
    /// Appareil d'où vient le relevé (marque et modèle, sans numéro de série).
    #[serde(default)]
    appareil: String,
    releve: String,
    attendu: Attendu,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attendu {
    total: Option<u64>,
    noir: Option<u64>,
    couleur: Option<u64>,
    /// Écart toléré entre noir + couleur et le total (pages), 0 par défaut.
    #[serde(default)]
    ecart_max: u64,
}

fn dossier() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("profils")
}

/// Lit une ligne `OID = TYPE: valeur` ; refuse ce qui n'est pas un nombre ou un OID.
fn ligne(l: &str) -> Result<(Oid, Valeur), String> {
    let (oid, reste) = l.split_once(" = ").ok_or("attendu « OID = TYPE: valeur »")?;
    let oid: Oid = oid
        .trim()
        .trim_start_matches('.')
        .parse()
        .map_err(|e: tirage_snmp::ber::ErreurBer| e.0)?;
    let (genre, valeur) = reste.split_once(':').ok_or("type manquant")?;
    let valeur = valeur.trim();
    // snmpwalk écrit parfois une énumération « nom(3) ».
    let nombre = || -> Result<i64, String> {
        let v = match (valeur.find('('), valeur.strip_suffix(')')) {
            (Some(i), Some(v)) => &v[i + 1..],
            _ => valeur,
        };
        v.parse().map_err(|_| format!("nombre illisible : {valeur}"))
    };
    let positif = || nombre().and_then(|n| u64::try_from(n).map_err(|_| "nombre négatif".to_owned()));
    let v = match genre.trim() {
        "INTEGER" => Valeur::Entier(nombre()?),
        "Counter32" => Valeur::Compteur32(u32::try_from(positif()?).map_err(|_| "hors Counter32")?),
        "Gauge32" => Valeur::Jauge32(u32::try_from(positif()?).map_err(|_| "hors Gauge32")?),
        "Counter64" => Valeur::Compteur64(positif()?),
        "OID" => Valeur::Oid(
            valeur
                .trim_start_matches('.')
                .parse()
                .map_err(|e: tirage_snmp::ber::ErreurBer| e.0)?,
        ),
        autre => {
            return Err(format!(
                "type « {autre} » refusé dans un relevé d'essai : ne garder que les nombres et l'OID de l'appareil"
            ));
        }
    };
    Ok((oid, v))
}

fn agent(releve: &str) -> Result<FauxAgent, String> {
    let mut a = FauxAgent::nouveau("essai");
    for (n, l) in releve.lines().enumerate() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (oid, v) = ligne(l).map_err(|e| format!("ligne {} : {e}", n + 1))?;
        a.mib.insert(oid, v);
    }
    Ok(a)
}

fn noms_de_fichiers(d: &Path) -> BTreeSet<String> {
    std::fs::read_dir(d)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .map(|p| p.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect()
}

#[test]
fn chaque_profil_a_un_essai_et_chaque_essai_un_profil() {
    let profils = noms_de_fichiers(&dossier());
    let essais = noms_de_fichiers(&dossier().join("essais"));
    assert_eq!(
        profils, essais,
        "profils/X.toml et profils/essais/X.toml vont par paire"
    );
}

#[test]
fn profils_bien_nommes_et_marques_distinctes() {
    let p = Profils::charger(None).unwrap();
    let mut entreprises = BTreeSet::new();
    for (fichier, texte) in INTEGRES {
        let profil = tirage_collecteur::copieurs::Profil::lire(texte).unwrap();
        assert_eq!(
            *fichier,
            format!("{}.toml", profil.nom),
            "le fichier porte le nom du profil"
        );
        assert!(
            profil
                .nom
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{} : nom en minuscules, chiffres et tirets",
            profil.nom
        );
        assert!(
            !profil.description.trim().is_empty(),
            "{} : description manquante",
            profil.nom
        );
        if let Some(e) = profil.entreprise {
            assert!(
                entreprises.insert(e),
                "deux profils pour le numéro d'entreprise {e}"
            );
        }
    }
    assert_eq!(p.noms().len(), INTEGRES.len());
}

#[test]
fn chaque_essai_redonne_les_compteurs_attendus() {
    let profils = Profils::charger(None).unwrap();
    let mut erreurs = Vec::new();
    for nom in noms_de_fichiers(&dossier().join("essais")) {
        let chemin = dossier().join("essais").join(format!("{nom}.toml"));
        let essai: Essai = match toml::from_str(&std::fs::read_to_string(&chemin).unwrap()) {
            Ok(e) => e,
            Err(e) => {
                erreurs.push(format!("{nom} : {e}"));
                continue;
            }
        };
        let mut a = match agent(&essai.releve) {
            Ok(a) => a,
            Err(e) => {
                erreurs.push(format!("{nom} : {e}"));
                continue;
            }
        };
        if essai.profil != nom {
            erreurs.push(format!("{nom} : l'essai doit viser le profil du même nom"));
        }
        let oid_appareil: Oid = SYS_OBJECT_ID.parse().unwrap();
        if !a.mib.contains_key(&oid_appareil) {
            erreurs.push(format!(
                "{nom} : ligne {SYS_OBJECT_ID} (identifiant de l'appareil) manquante"
            ));
            continue;
        }
        let (r, contexte) = relever(
            &mut Client::nouveau(&mut a, b"essai"),
            "essai",
            "COPIEUR-EXEMPLE",
            &profils,
            None,
            0,
        )
        .unwrap_or_else(|e| panic!("{nom} : {e}"));
        let quoi = if essai.appareil.is_empty() {
            nom.clone()
        } else {
            format!("{nom} ({})", essai.appareil)
        };
        if contexte.profil != essai.profil {
            erreurs.push(format!(
                "{quoi} : profil choisi {}, attendu {}",
                contexte.profil, essai.profil
            ));
        }
        let att = &essai.attendu;
        for (compteur, obtenu, voulu) in [
            ("total", r.compteur_total, att.total),
            ("noir", r.compteur_noir, att.noir),
            ("couleur", r.compteur_couleur, att.couleur),
        ] {
            if obtenu != voulu {
                erreurs.push(format!("{quoi} : {compteur} = {obtenu:?}, attendu {voulu:?}"));
            }
        }
        if let (Some(t), Some(n), Some(c)) = (att.total, att.noir, att.couleur)
            && t.abs_diff(n + c) > att.ecart_max
        {
            erreurs.push(format!(
                "{quoi} : noir + couleur = {}, total = {t} (écart toléré {})",
                n + c,
                att.ecart_max
            ));
        }
    }
    assert!(erreurs.is_empty(), "\n{}", erreurs.join("\n"));
}

#[test]
fn releve_d_essai_refuse_le_texte_et_les_adresses() {
    assert!(ligne("1.3.6.1.2.1.1.5.0 = STRING: \"COPIEUR-1\"").is_err());
    assert!(ligne("1.3.6.1.2.1.43.5.1.1.17.1 = Hex-STRING: 41 42").is_err());
    assert!(ligne("1.3.6.1.2.1.4.20.1.1.192.0.2.1 = IpAddress: 192.0.2.1").is_err());
    assert!(ligne("1.3.6.1.2.1.1.3.0 = Timeticks: (12) 0:00:00.12").is_err());
    assert_eq!(
        ligne(".1.3.6.1.2.1.43.10.2.1.4.1.1 = Counter32: 42").unwrap(),
        (
            "1.3.6.1.2.1.43.10.2.1.4.1.1".parse().unwrap(),
            Valeur::Compteur32(42)
        )
    );
    assert_eq!(
        ligne("1.3.6.1.2.1.25.3.5.1.1.1 = INTEGER: idle(3)").unwrap().1,
        Valeur::Entier(3)
    );
    assert_eq!(
        ligne("1.3.6.1.2.1.1.2.0 = OID: .1.3.6.1.4.1.32473.1").unwrap().1,
        Valeur::Oid("1.3.6.1.4.1.32473.1".parse().unwrap())
    );
}
