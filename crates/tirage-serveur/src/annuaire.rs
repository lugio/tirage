//! Annuaire, en lecture seule : connexion réservée à un groupe, service et cabinet des personnes.
//!
//! - Seules les personnes **qui ont imprimé** sont lues, par recherche nominative : l'annuaire n'est jamais recopié.
//! - Un mot de passe vide est refusé avant tout échange : en LDAP, il donnerait une liaison anonyme « réussie ».
//! - Le groupe est vérifié avant le mot de passe, sur le compte de lecture : un compte hors groupe ne fait jamais
//!   monter le compteur d'échecs de l'annuaire.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ldap3::{LdapConn, LdapConnSettings, LdapError, Scope, SearchEntry, SearchOptions, ldap_escape};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use crate::base::Base;
use crate::config::ConfigAnnuaire;

const DELAI: Duration = Duration::from_secs(10);
/// Règle de l'annuaire Active Directory : appartenance directe ou par groupes imbriqués.
const REGLE_CHAINE: &str = "1.2.840.113556.1.4.1941";
/// Identifiants demandés par recherche.
const PAQUET: usize = 50;
/// Code LDAP « identifiants invalides ».
const IDENTIFIANTS_INVALIDES: u32 = 49;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identite {
    /// Identifiant tel qu'écrit dans l'annuaire.
    pub login: String,
    pub nom: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fiche {
    pub identifiant: String,
    pub nom: Option<String>,
    pub service: Option<String>,
    pub cabinet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErreurAuth {
    /// Mot de passe faux, compte inconnu ou hors du groupe : indiscernables pour celui qui se connecte.
    Refus,
    /// Annuaire injoignable ou mal configuré. Le texte ne contient ni identifiant ni mot de passe.
    Indisponible(String),
}

pub trait Annuaire: Send + Sync {
    fn authentifier(&self, login: &str, mot_de_passe: &str) -> Result<Identite, ErreurAuth>;
    /// Fiches des identifiants donnés ; un identifiant absent de l'annuaire n'a simplement pas de fiche.
    fn fiches(&self, identifiants: &[String]) -> Result<Vec<Fiche>, String>;
}

/// Identifiant nu : `DOMAINE\nom` et `nom@domaine` deviennent `nom`.
pub fn identifiant_nu(utilisateur: &str) -> &str {
    let u = utilisateur.rsplit('\\').next().unwrap_or(utilisateur);
    u.split('@').next().unwrap_or(u).trim()
}

/// Première unité d'organisation d'un DN (`CN=x,OU=Audit,OU=Lyon,DC=…` → `Audit`).
fn premiere_ou(dn: &str) -> Option<String> {
    let mut reste = dn;
    while let Some((debut, suite)) = coupe_rdn(reste) {
        if let Some((a, v)) = debut.split_once('=')
            && a.trim().eq_ignore_ascii_case("ou")
        {
            return Some(v.trim().replace(r"\,", ",")).filter(|s| !s.is_empty());
        }
        reste = suite;
    }
    None
}

/// Coupe le premier élément d'un DN, à la première virgule non échappée.
fn coupe_rdn(dn: &str) -> Option<(&str, &str)> {
    if dn.is_empty() {
        return None;
    }
    let mut echappe = false;
    for (i, c) in dn.char_indices() {
        match c {
            ',' if !echappe => return Some((&dn[..i], &dn[i + 1..])),
            '\\' => echappe = !echappe,
            _ => echappe = false,
        }
    }
    Some((dn, ""))
}

/// Annuaire réel, par LDAPS.
pub struct Ldap {
    cfg: ConfigAnnuaire,
    mot_de_passe: String,
    tls: Option<Arc<rustls::ClientConfig>>,
}

impl Ldap {
    pub fn nouveau(cfg: &ConfigAnnuaire) -> Result<Self, String> {
        let brut = std::fs::read_to_string(&cfg.mot_de_passe_fichier)
            .map_err(|e| format!("{} : {e}", cfg.mot_de_passe_fichier.display()))?;
        let mot_de_passe = brut.trim_end_matches(['\r', '\n']).to_owned();
        if mot_de_passe.is_empty() {
            return Err(format!("{} : fichier vide", cfg.mot_de_passe_fichier.display()));
        }
        let tls = cfg.ca.as_deref().map(config_tls).transpose()?;
        Ok(Ldap {
            cfg: cfg.clone(),
            mot_de_passe,
            tls,
        })
    }

    fn connecter(&self) -> Result<LdapConn, LdapError> {
        let mut r = LdapConnSettings::new().set_conn_timeout(DELAI);
        if let Some(t) = &self.tls {
            r = r.set_config(Arc::clone(t));
        }
        LdapConn::with_settings(r, &self.cfg.url)
    }

    /// Connexion ouverte sous le compte de lecture.
    fn connecter_lecture(&self) -> Result<LdapConn, String> {
        let mut c = self.connecter().map_err(texte_erreur)?;
        let r = c
            .with_timeout(DELAI)
            .simple_bind(&self.cfg.compte, &self.mot_de_passe)
            .map_err(texte_erreur)?;
        if r.rc != 0 {
            return Err(format!("compte de lecture refusé (code {})", r.rc));
        }
        Ok(c)
    }

    fn attributs(&self) -> Vec<&str> {
        let mut a = vec![
            self.cfg.attribut_identifiant.as_str(),
            self.cfg.attribut_nom.as_str(),
        ];
        for x in [&self.cfg.attribut_service, &self.cfg.attribut_cabinet] {
            if x != "dn:ou" {
                a.push(x);
            }
        }
        a
    }

    fn valeur(&self, e: &SearchEntry, attribut: &str) -> Option<String> {
        if attribut == "dn:ou" {
            return premiere_ou(&e.dn);
        }
        // L'annuaire rend le nom d'attribut dans sa propre casse, pas forcément celle de la configuration.
        e.attrs
            .iter()
            .find(|(nom, _)| nom.eq_ignore_ascii_case(attribut))
            .and_then(|(_, v)| v.first())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    fn fiche(&self, e: &SearchEntry) -> Option<Fiche> {
        Some(Fiche {
            identifiant: self.valeur(e, &self.cfg.attribut_identifiant)?,
            nom: self.valeur(e, &self.cfg.attribut_nom),
            service: self.valeur(e, &self.cfg.attribut_service),
            cabinet: self.valeur(e, &self.cfg.attribut_cabinet),
        })
    }

    /// Recherche ; les renvois vers d'autres serveurs (fréquents à la racine d'un domaine) sont ignorés.
    fn chercher(&self, c: &mut LdapConn, filtre: &str, limite: i32) -> Result<Vec<SearchEntry>, String> {
        let (entrees, _) = c
            .with_timeout(DELAI)
            .with_search_options(SearchOptions::new().sizelimit(limite))
            .search(&self.cfg.base, Scope::Subtree, filtre, self.attributs())
            .and_then(ldap3::SearchResult::success)
            .map_err(texte_erreur)?;
        Ok(entrees
            .into_iter()
            .filter(|e| !e.is_ref() && !e.is_intermediate())
            .map(SearchEntry::construct)
            .collect())
    }
}

impl Annuaire for Ldap {
    fn authentifier(&self, login: &str, mot_de_passe: &str) -> Result<Identite, ErreurAuth> {
        let login = identifiant_nu(login);
        if login.is_empty() || mot_de_passe.is_empty() {
            return Err(ErreurAuth::Refus);
        }
        let mut c = self.connecter_lecture().map_err(ErreurAuth::Indisponible)?;
        let filtre = format!(
            "(&{}({}={})(memberOf:{REGLE_CHAINE}:={}))",
            self.cfg.filtre_personnes,
            self.cfg.attribut_identifiant,
            ldap_escape(login),
            ldap_escape(&self.cfg.groupe_autorise),
        );
        let entrees = self
            .chercher(&mut c, &filtre, 2)
            .map_err(ErreurAuth::Indisponible)?;
        let _ = c.unbind();
        let [e] = entrees.as_slice() else {
            return Err(ErreurAuth::Refus);
        };
        let fiche = self.fiche(e).ok_or(ErreurAuth::Refus)?;

        // Le mot de passe se vérifie sur une connexion à part, jamais réutilisée.
        let mut c = self
            .connecter()
            .map_err(|e| ErreurAuth::Indisponible(texte_erreur(e)))?;
        let r = c
            .with_timeout(DELAI)
            .simple_bind(&e.dn, mot_de_passe)
            .map_err(|e| ErreurAuth::Indisponible(texte_erreur(e)))?;
        let _ = c.unbind();
        match r.rc {
            0 => Ok(Identite {
                login: fiche.identifiant,
                nom: fiche.nom,
            }),
            IDENTIFIANTS_INVALIDES => Err(ErreurAuth::Refus),
            code => Err(ErreurAuth::Indisponible(format!(
                "vérification du mot de passe : code {code}"
            ))),
        }
    }

    fn fiches(&self, identifiants: &[String]) -> Result<Vec<Fiche>, String> {
        let mut c = self.connecter_lecture()?;
        let mut tout = Vec::with_capacity(identifiants.len());
        for paquet in identifiants.chunks(PAQUET) {
            let membres: String = paquet
                .iter()
                .map(|i| format!("({}={})", self.cfg.attribut_identifiant, ldap_escape(i.as_str())))
                .collect();
            let filtre = format!("(&{}(|{membres}))", self.cfg.filtre_personnes);
            let entrees = self.chercher(&mut c, &filtre, 0)?;
            tout.extend(entrees.iter().filter_map(|e| self.fiche(e)));
        }
        let _ = c.unbind();
        Ok(tout)
    }
}

/// Erreur LDAP sans détail du serveur : seul son type et son code passent, jamais le contenu d'une requête.
fn texte_erreur(e: LdapError) -> String {
    match e {
        LdapError::LdapResult { result } => format!("réponse de l'annuaire : code {}", result.rc),
        LdapError::Timeout { .. } => "délai dépassé".into(),
        LdapError::FilterParsing => "filtre LDAP invalide (voir filtre_personnes)".into(),
        autre => format!("connexion à l'annuaire : {autre}"),
    }
}

fn config_tls(ca: &Path) -> Result<Arc<rustls::ClientConfig>, String> {
    let mut racines = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(ca).map_err(|e| format!("{} : {e}", ca.display()))? {
        let cert = cert.map_err(|e| format!("{} : {e}", ca.display()))?;
        racines.add(cert).map_err(|e| format!("{} : {e}", ca.display()))?;
    }
    if racines.is_empty() {
        return Err(format!("{} : aucun certificat", ca.display()));
    }
    let c = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(racines)
        .with_no_client_auth();
    Ok(Arc::new(c))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Bilan {
    pub verifies: usize,
    pub absents: usize,
}

/// Met à jour les fiches des personnes vues dans les travaux. Le verrou de la base n'est jamais tenu pendant un
/// échange avec l'annuaire. Annuaire injoignable : rien n'est modifié, on réessaiera.
pub fn synchroniser(
    base: &Mutex<Base>,
    annuaire: &dyn Annuaire,
    maintenant_ms: i64,
    age_max_ms: i64,
) -> Result<Bilan, String> {
    let verrou = || base.lock().map_err(|_| "base indisponible".to_owned());
    let utilisateurs = verrou()?
        .utilisateurs_a_verifier(maintenant_ms - age_max_ms)
        .map_err(|e| e.to_string())?;
    let mut bilan = Bilan::default();
    for paquet in utilisateurs.chunks(PAQUET) {
        let nus: Vec<String> = paquet.iter().map(|u| identifiant_nu(u).to_owned()).collect();
        let fiches: HashMap<String, Fiche> = annuaire
            .fiches(&nus)?
            .into_iter()
            .map(|f| (f.identifiant.to_lowercase(), f))
            .collect();
        let b = verrou()?;
        for u in paquet {
            bilan.verifies += 1;
            let r = match fiches.get(&identifiant_nu(u).to_lowercase()) {
                Some(f) => b.personne_trouvee(
                    u,
                    f.nom.as_deref(),
                    f.service.as_deref(),
                    f.cabinet.as_deref(),
                    maintenant_ms,
                ),
                None => {
                    bilan.absents += 1;
                    b.personne_absente(u, maintenant_ms)
                }
            };
            r.map_err(|e| e.to_string())?;
        }
    }
    Ok(bilan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::tests::travail;
    use tirage_modele::Evenement;

    struct Faux(Result<Vec<Fiche>, String>);
    impl Annuaire for Faux {
        fn authentifier(&self, _: &str, _: &str) -> Result<Identite, ErreurAuth> {
            Err(ErreurAuth::Refus)
        }
        fn fiches(&self, _: &[String]) -> Result<Vec<Fiche>, String> {
            self.0.clone()
        }
    }

    #[test]
    fn identifiants() {
        assert_eq!(identifiant_nu(r"EXEMPLE\a.exemple"), "a.exemple");
        assert_eq!(identifiant_nu("a.exemple@example.org"), "a.exemple");
        assert_eq!(identifiant_nu(" a.exemple "), "a.exemple");
        assert_eq!(identifiant_nu(""), "");
    }

    #[test]
    fn unite_du_dn() {
        assert_eq!(
            premiere_ou("CN=A,OU=Audit,OU=Lyon,DC=example,DC=org").as_deref(),
            Some("Audit")
        );
        assert_eq!(
            premiere_ou(r"CN=Exemple\, A,OU=Paie\, Social,DC=x").as_deref(),
            Some("Paie, Social")
        );
        assert_eq!(premiere_ou("CN=A,DC=example,DC=org"), None);
    }

    #[test]
    fn synchro_trouves_absents_et_panne() {
        let base = Mutex::new(Base::en_memoire().unwrap());
        {
            let mut b = base.lock().unwrap();
            let mut t1 = travail(1);
            t1.utilisateur = r"EXEMPLE\A.Exemple".into();
            b.enregistrer(&Evenement::Travail(t1), 0).unwrap();
            let mut t2 = travail(2);
            t2.utilisateur = "b.exemple".into();
            b.enregistrer(&Evenement::Travail(t2), 0).unwrap();
        }
        // Panne : aucune fiche écrite, tout reste à vérifier.
        assert!(synchroniser(&base, &Faux(Err("panne".into())), 1000, 100).is_err());
        assert_eq!(
            base.lock().unwrap().utilisateurs_a_verifier(999).unwrap().len(),
            2
        );

        let fiche = Fiche {
            identifiant: "a.exemple".into(),
            nom: Some("A Exemple".into()),
            service: Some("Audit".into()),
            cabinet: Some("Exemple-Est".into()),
        };
        let bilan = synchroniser(&base, &Faux(Ok(vec![fiche])), 1000, 100).unwrap();
        assert_eq!(
            bilan,
            Bilan {
                verifies: 2,
                absents: 1
            }
        );
        let b = base.lock().unwrap();
        let p = b.personne(r"exemple\a.exemple").unwrap().unwrap();
        assert_eq!(p.service.as_deref(), Some("Audit"));
        assert!(!b.personne("b.exemple").unwrap().unwrap().trouve);
        drop(b);
        // Fiches récentes : pas de nouvelle lecture.
        let bilan = synchroniser(&base, &Faux(Err("panne".into())), 1050, 100).unwrap();
        assert_eq!(bilan.verifies, 0);
    }
}
