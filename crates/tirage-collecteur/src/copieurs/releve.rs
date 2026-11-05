//! Relevé SNMP d'un copieur : identité, compteurs (selon le profil), consommables, état et alertes.
//!
//! OID standard utilisés : MIB-II `system` (RFC 1213), Host-Resources (RFC 2790), Printer-MIB (RFC 3805).

use std::collections::BTreeMap;

use tirage_modele::{Consommable, ReleveCopieur};
use tirage_snmp::{Client, ErreurSnmp, Oid, Transport, Valeur};

use super::profil::{Profil, Profils};

fn oid(s: &str) -> Oid {
    s.parse().expect("OID standard valide")
}

pub mod oids {
    pub const SYS_DESCR: &str = "1.3.6.1.2.1.1.1.0";
    pub const SYS_OBJECT_ID: &str = "1.3.6.1.2.1.1.2.0";
    pub const HR_DEVICE_DESCR: &str = "1.3.6.1.2.1.25.3.2.1.3";
    pub const HR_PRINTER_STATUS: &str = "1.3.6.1.2.1.25.3.5.1.1";
    pub const HR_PRINTER_ERREURS: &str = "1.3.6.1.2.1.25.3.5.1.2";
    pub const PRT_NUMERO_SERIE: &str = "1.3.6.1.2.1.43.5.1.1.17";
    pub const PRT_CONSO_DESCRIPTION: &str = "1.3.6.1.2.1.43.11.1.1.6";
    pub const PRT_CONSO_CAPACITE: &str = "1.3.6.1.2.1.43.11.1.1.8";
    pub const PRT_CONSO_NIVEAU: &str = "1.3.6.1.2.1.43.11.1.1.9";
    pub const PRT_ALERTE_DESCRIPTION: &str = "1.3.6.1.2.1.43.18.1.1.8";
}

/// Garde-fou : nombre maximal de lignes lues par table.
const LIGNES_MAX: usize = 200;

/// `hrPrinterStatus` (RFC 2790).
fn etat_imprimante(n: i64) -> &'static str {
    match n {
        3 => "prêt",
        4 => "impression",
        5 => "préchauffage",
        2 => "inconnu",
        _ => "autre",
    }
}

/// `hrPrinterDetectedErrorState` : chaîne de bits, bit 0 = bit de poids fort du premier octet (RFC 2790).
const ERREURS: [&str; 16] = [
    "papier bas",
    "plus de papier",
    "toner bas",
    "plus de toner",
    "porte ouverte",
    "bourrage",
    "hors ligne",
    "intervention demandée",
    "bac d'entrée absent",
    "bac de sortie absent",
    "consommable absent",
    "bac de sortie presque plein",
    "bac de sortie plein",
    "bac d'entrée vide",
    "maintenance préventive dépassée",
    "",
];

fn erreurs_detectees(octets: &[u8]) -> Vec<String> {
    let mut v = Vec::new();
    for (i, libelle) in ERREURS.iter().enumerate() {
        let (octet, bit) = (i / 8, 7 - (i % 8));
        if !libelle.is_empty() && octets.get(octet).is_some_and(|b| b >> bit & 1 == 1) {
            v.push((*libelle).to_owned());
        }
    }
    v
}

/// Index de ligne d'une colonne de table : la fin de l'OID après la colonne.
fn index(base: &Oid, o: &Oid) -> Vec<u32> {
    o.0[base.0.len()..].to_vec()
}

fn colonne<T: Transport>(c: &mut Client<T>, base: &str) -> Result<BTreeMap<Vec<u32>, Valeur>, ErreurSnmp> {
    let b = oid(base);
    Ok(c.parcourir(&b, LIGNES_MAX)?
        .into_iter()
        .map(|(o, v)| (index(&b, &o), v))
        .collect())
}

/// Somme des OID d'un compteur ; `None` si la liste est vide ou si un seul manque (pas de total partiel).
/// Avec `absents_toleres`, un OID absent ou négatif compte pour zéro, et `None` seulement si aucun ne répond.
fn somme<T: Transport>(
    c: &mut Client<T>,
    liste: &[Oid],
    absents_toleres: bool,
) -> Result<Option<u64>, ErreurSnmp> {
    if liste.is_empty() {
        return Ok(None);
    }
    let mut total: u64 = 0;
    let mut trouve = false;
    for (_, v) in c.get(liste)? {
        match v.nombre().and_then(|n| u64::try_from(n).ok()) {
            Some(n) => {
                total = total.saturating_add(n);
                trouve = true;
            }
            None if absents_toleres => {}
            None => return Ok(None),
        }
    }
    Ok(trouve.then_some(total))
}

/// Ce qu'un relevé a utilisé, pour l'affichage d'essai.
#[derive(Debug, Clone, PartialEq)]
pub struct Contexte {
    pub profil: String,
    pub sys_object_id: Option<Oid>,
}

/// Relève un copieur. `profil_force` : nom d'un profil imposé par la configuration.
pub fn relever<T: Transport>(
    c: &mut Client<T>,
    collecteur: &str,
    copieur: &str,
    profils: &Profils,
    profil_force: Option<&str>,
    maintenant_ms: i64,
) -> Result<(ReleveCopieur, Contexte), ErreurSnmp> {
    let ident = c.get(&[oid(oids::SYS_DESCR), oid(oids::SYS_OBJECT_ID)])?;
    let sys_descr = ident[0].1.texte();
    let sys_object_id = match &ident[1].1 {
        Valeur::Oid(o) => Some(o.clone()),
        _ => None,
    };
    let profil: &Profil = match profil_force {
        Some(nom) => profils
            .par_nom(nom)
            .ok_or_else(|| ErreurSnmp::Inattendue(format!("profil inconnu : {nom}")))?,
        None => profils.choisir(sys_object_id.as_ref()),
    };

    let modele = c
        .premiere(&oid(oids::HR_DEVICE_DESCR))?
        .and_then(|(_, v)| v.texte())
        .or(sys_descr);
    let numero_serie = c
        .premiere(&oid(oids::PRT_NUMERO_SERIE))?
        .and_then(|(_, v)| v.texte());

    let total_oids = if profil.compteurs.total.is_empty() {
        &profils.repli().compteurs.total
    } else {
        &profil.compteurs.total
    };
    let tolere = profil.compteurs.absents_toleres;
    let compteur_total = somme(c, total_oids, false)?;
    let compteur_noir = somme(c, &profil.compteurs.noir, tolere)?;
    let compteur_couleur = somme(c, &profil.compteurs.couleur, tolere)?;

    let descriptions = colonne(c, oids::PRT_CONSO_DESCRIPTION)?;
    let capacites = colonne(c, oids::PRT_CONSO_CAPACITE)?;
    let niveaux = colonne(c, oids::PRT_CONSO_NIVEAU)?;
    let consommables = descriptions
        .iter()
        .map(|(i, d)| Consommable {
            description: d
                .texte()
                .unwrap_or_else(|| format!("consommable {}", Oid(i.clone()))),
            niveau: niveaux.get(i).and_then(Valeur::nombre),
            capacite: capacites.get(i).and_then(Valeur::nombre),
        })
        .collect();

    let etat = c
        .premiere(&oid(oids::HR_PRINTER_STATUS))?
        .and_then(|(_, v)| v.nombre())
        .map(|n| etat_imprimante(n).to_owned());

    let mut alertes = match c.premiere(&oid(oids::HR_PRINTER_ERREURS))? {
        Some((_, Valeur::Octets(o))) => erreurs_detectees(&o),
        _ => Vec::new(),
    };
    for v in colonne(c, oids::PRT_ALERTE_DESCRIPTION)?.values() {
        if let Some(t) = v.texte()
            && !alertes.contains(&t)
        {
            alertes.push(t);
        }
    }

    Ok((
        ReleveCopieur {
            collecteur: collecteur.to_owned(),
            copieur: copieur.to_owned(),
            releve_ms: maintenant_ms,
            modele,
            numero_serie,
            compteur_total,
            compteur_noir,
            compteur_couleur,
            consommables,
            etat,
            alertes,
        },
        Contexte {
            profil: profil.nom.clone(),
            sys_object_id,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tirage_snmp::client::essai::FauxAgent;

    fn texte(s: &str) -> Valeur {
        Valeur::Octets(s.as_bytes().to_vec())
    }

    /// Copieur fictif de la marque « 99999 », conforme aux MIB standard.
    fn copieur() -> FauxAgent {
        FauxAgent::nouveau("exemple")
            .avec(oids::SYS_DESCR, texte("EXEMPLE Copieur 3000"))
            .avec(
                oids::SYS_OBJECT_ID,
                Valeur::Oid("1.3.6.1.4.1.99999.1.7".parse().unwrap()),
            )
            .avec("1.3.6.1.2.1.25.3.2.1.3.1", texte("EXEMPLE C3000 PCL"))
            .avec("1.3.6.1.2.1.25.3.5.1.1.1", Valeur::Entier(3))
            .avec(
                "1.3.6.1.2.1.25.3.5.1.2.1",
                Valeur::Octets(vec![0b0010_0100, 0b0000_0000]),
            ) // toner bas + bourrage
            .avec("1.3.6.1.2.1.43.5.1.1.17.1", texte("SN-000000"))
            .avec("1.3.6.1.2.1.43.10.2.1.4.1.1", Valeur::Compteur32(123_456))
            .avec("1.3.6.1.2.1.43.11.1.1.6.1.1", texte("Toner noir"))
            .avec("1.3.6.1.2.1.43.11.1.1.6.1.2", texte("Toner cyan"))
            .avec("1.3.6.1.2.1.43.11.1.1.8.1.1", Valeur::Entier(100))
            .avec("1.3.6.1.2.1.43.11.1.1.8.1.2", Valeur::Entier(100))
            .avec("1.3.6.1.2.1.43.11.1.1.9.1.1", Valeur::Entier(42))
            .avec("1.3.6.1.2.1.43.11.1.1.9.1.2", Valeur::Entier(-3))
            .avec("1.3.6.1.2.1.43.18.1.1.8.1.1", texte("Bourrage bac 2"))
            .avec("1.3.6.1.4.1.99999.10.1", Valeur::Compteur32(100_000))
            .avec("1.3.6.1.4.1.99999.10.2", Valeur::Compteur32(20_000))
            .avec("1.3.6.1.4.1.99999.10.3", Valeur::Compteur32(3_456))
    }

    fn profils_avec_exemple() -> Profils {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("exemple.toml"),
            "nom = \"exemple\"\nentreprise = 99999\n[compteurs]\nnoir = [\"1.3.6.1.4.1.99999.10.1\", \"1.3.6.1.4.1.99999.10.2\"]\ncouleur = [\"1.3.6.1.4.1.99999.10.3\"]\n",
        )
        .unwrap();
        Profils::charger(Some(d.path())).unwrap()
    }

    #[test]
    fn releve_complet_avec_profil_de_marque() {
        let mut a = copieur();
        let mut c = Client::nouveau(&mut a, b"exemple");
        let (r, ctx) = relever(
            &mut c,
            "impression-1",
            "COPIEUR-EXEMPLE-1",
            &profils_avec_exemple(),
            None,
            1_000,
        )
        .unwrap();
        assert_eq!(ctx.profil, "exemple");
        assert_eq!(r.modele.as_deref(), Some("EXEMPLE C3000 PCL"));
        assert_eq!(r.numero_serie.as_deref(), Some("SN-000000"));
        assert_eq!(
            (r.compteur_total, r.compteur_noir, r.compteur_couleur),
            (Some(123_456), Some(120_000), Some(3_456))
        );
        assert_eq!(r.consommables.len(), 2);
        assert_eq!(r.consommables[0].pourcentage(), Some(42));
        assert_eq!(r.consommables[1].pourcentage(), None, "-3 = niveau inconnu");
        assert_eq!(r.etat.as_deref(), Some("prêt"));
        assert_eq!(r.alertes, vec!["toner bas", "bourrage", "Bourrage bac 2"]);
    }

    #[test]
    fn appareil_inconnu_repli_standard() {
        let mut a = copieur();
        let mut c = Client::nouveau(&mut a, b"exemple");
        let (r, ctx) = relever(&mut c, "c", "X", &Profils::charger(None).unwrap(), None, 0).unwrap();
        assert_eq!(ctx.profil, "printer-mib");
        assert_eq!(
            (r.compteur_total, r.compteur_noir, r.compteur_couleur),
            (Some(123_456), None, None)
        );
    }

    #[test]
    fn profil_konica_sans_compteurs_prives_garde_le_total_standard() {
        let mut a = copieur().avec(
            oids::SYS_OBJECT_ID,
            Valeur::Oid("1.3.6.1.4.1.18334.1.1".parse().unwrap()),
        );
        let mut c = Client::nouveau(&mut a, b"exemple");
        let (r, ctx) = relever(&mut c, "c", "X", &Profils::charger(None).unwrap(), None, 0).unwrap();
        assert_eq!(ctx.profil, "konica-minolta");
        assert_eq!(r.compteur_total, Some(123_456));
        assert_eq!(r.compteur_noir, None);
    }

    const NOIR_COPIE: &str = "1.3.6.1.4.1.18334.1.1.1.5.7.2.2.1.5.1.1";
    const NOIR_IMPRESSION: &str = "1.3.6.1.4.1.18334.1.1.1.5.7.2.2.1.5.1.2";
    const COULEUR_COPIE: &str = "1.3.6.1.4.1.18334.1.1.1.5.7.2.2.1.5.2.1";
    const COULEUR_IMPRESSION: &str = "1.3.6.1.4.1.18334.1.1.1.5.7.2.2.1.5.2.2";
    const UNE_COULEUR_IMPRESSION: &str = "1.3.6.1.4.1.18334.1.1.1.5.7.2.2.1.5.3.2";
    const DEUX_COULEURS_IMPRESSION: &str = "1.3.6.1.4.1.18334.1.1.1.5.7.2.2.1.5.4.2";

    fn konica() -> FauxAgent {
        copieur().avec(
            oids::SYS_OBJECT_ID,
            Valeur::Oid("1.3.6.1.4.1.18334.1.1".parse().unwrap()),
        )
    }

    #[test]
    fn konica_multifonction_additionne_copie_et_impression() {
        let mut a = konica()
            .avec(NOIR_COPIE, Valeur::Entier(10))
            .avec(NOIR_IMPRESSION, Valeur::Entier(200))
            .avec(COULEUR_COPIE, Valeur::Entier(3))
            .avec(COULEUR_IMPRESSION, Valeur::Entier(40))
            .avec(UNE_COULEUR_IMPRESSION, Valeur::Entier(-1))
            .avec(DEUX_COULEURS_IMPRESSION, Valeur::Entier(5));
        let mut c = Client::nouveau(&mut a, b"exemple");
        let (r, _) = relever(&mut c, "c", "X", &Profils::charger(None).unwrap(), None, 0).unwrap();
        assert_eq!((r.compteur_noir, r.compteur_couleur), (Some(210), Some(48)));
    }

    #[test]
    fn konica_sans_fonction_copie_garde_ses_compteurs() {
        let mut a = konica()
            .avec(NOIR_IMPRESSION, Valeur::Entier(232))
            .avec(COULEUR_IMPRESSION, Valeur::Entier(36));
        let mut c = Client::nouveau(&mut a, b"exemple");
        let (r, _) = relever(&mut c, "c", "X", &Profils::charger(None).unwrap(), None, 0).unwrap();
        assert_eq!((r.compteur_noir, r.compteur_couleur), (Some(232), Some(36)));
    }

    #[test]
    fn compteur_partiel_jamais_additionne() {
        let mut a = copieur();
        a.mib.remove(&"1.3.6.1.4.1.99999.10.2".parse::<Oid>().unwrap());
        let mut c = Client::nouveau(&mut a, b"exemple");
        let (r, _) = relever(&mut c, "c", "X", &profils_avec_exemple(), None, 0).unwrap();
        assert_eq!(r.compteur_noir, None);
        assert_eq!(r.compteur_couleur, Some(3_456));
    }

    #[test]
    fn appareil_minimal() {
        let mut a = FauxAgent::nouveau("exemple").avec(oids::SYS_DESCR, texte("Imprimante EXEMPLE"));
        let mut c = Client::nouveau(&mut a, b"exemple");
        let (r, _) = relever(&mut c, "c", "X", &Profils::charger(None).unwrap(), None, 0).unwrap();
        assert_eq!(r.modele.as_deref(), Some("Imprimante EXEMPLE"));
        assert!(r.consommables.is_empty() && r.alertes.is_empty());
        assert_eq!((r.compteur_total, r.etat), (None, None));
    }

    #[test]
    fn profil_impose_inconnu() {
        let mut a = copieur();
        let mut c = Client::nouveau(&mut a, b"exemple");
        assert!(
            relever(
                &mut c,
                "c",
                "X",
                &Profils::charger(None).unwrap(),
                Some("nulle-part"),
                0
            )
            .is_err()
        );
    }

    #[test]
    fn bits_d_erreur() {
        assert_eq!(erreurs_detectees(&[0x80]), vec!["papier bas"]);
        assert_eq!(
            erreurs_detectees(&[0x00, 0x02]),
            vec!["maintenance préventive dépassée"]
        );
        assert!(erreurs_detectees(&[]).is_empty());
    }
}
