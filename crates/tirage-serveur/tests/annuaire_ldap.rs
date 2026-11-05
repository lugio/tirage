//! Le vrai client LDAP contre un petit serveur LDAP fictif (TCP local, avec ou sans TLS) : identités et groupe
//! inventés, aucun annuaire réel.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tirage_serveur::annuaire::{Annuaire, ErreurAuth, Ldap};
use tirage_serveur::config::Config;

const COMPTE: &str = "CN=lecture,DC=example,DC=org";
const MDP_COMPTE: &str = "mot-de-passe-du-compte-fictif";
const GROUPE: &str = "CN=Lecteurs,OU=Groupes,DC=example,DC=org";
const DN_MEMBRE: &str = "CN=Membre,OU=Audit,DC=example,DC=org";
const MDP_MEMBRE: &str = "mot-de-passe-fictif";
const DN_SPECIAL: &str = "CN=Membre (Paie)*,OU=Audit,DC=example,DC=org";
const REGLE_CHAINE: &str = "1.2.840.113556.1.4.1941";

// --- BER minimal ---

fn longueur(n: usize) -> Vec<u8> {
    if n < 0x80 {
        vec![n as u8]
    } else {
        let o: Vec<u8> = n.to_be_bytes().iter().copied().skip_while(|b| *b == 0).collect();
        let mut r = vec![0x80 | o.len() as u8];
        r.extend(o);
        r
    }
}

fn tlv(tag: u8, valeur: &[u8]) -> Vec<u8> {
    let mut r = vec![tag];
    r.extend(longueur(valeur.len()));
    r.extend_from_slice(valeur);
    r
}

fn texte(s: &str) -> Vec<u8> {
    tlv(0x04, s.as_bytes())
}

/// Lit un élément au début de `o` : (tag, valeur, reste).
fn lire(o: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, o) = o.split_first()?;
    let (&l, mut o) = o.split_first()?;
    let n = if l < 0x80 {
        usize::from(l)
    } else {
        let k = usize::from(l & 0x7f);
        let n = o.get(..k)?.iter().fold(0usize, |a, b| a << 8 | usize::from(*b));
        o = &o[k..];
        n
    };
    Some((tag, o.get(..n)?, &o[n..]))
}

fn message(id: &[u8], op: Vec<u8>) -> Vec<u8> {
    let mut c = tlv(0x02, id);
    c.extend(op);
    tlv(0x30, &c)
}

fn resultat(tag: u8, code: u8) -> Vec<u8> {
    let mut c = tlv(0x0a, &[code]);
    c.extend(texte(""));
    c.extend(texte(""));
    tlv(tag, &c)
}

fn entree(dn: &str, attrs: &[(&str, &str)]) -> Vec<u8> {
    let mut liste = Vec::new();
    for (a, v) in attrs {
        let mut un = texte(a);
        un.extend(tlv(0x31, &texte(v)));
        liste.extend(tlv(0x30, &un));
    }
    let mut c = texte(dn);
    c.extend(tlv(0x30, &liste));
    tlv(0x64, &c)
}

// --- Serveur fictif ---

#[derive(Default)]
struct Trace {
    /// DN des liaisons reçues (jamais les mots de passe).
    liaisons: Vec<String>,
    /// Filtres bruts reçus (encodés BER).
    filtres: Vec<Vec<u8>>,
    /// Base et portée de chaque recherche (0 : l'objet seul).
    recherches: Vec<(String, u8)>,
}

fn contient(foin: &[u8], aiguille: &str) -> bool {
    foin.windows(aiguille.len()).any(|w| w == aiguille.as_bytes())
}

/// Égalités d'un filtre encodé (`&` et `|` parcourus) : (attribut, valeur), comme un vrai serveur les lirait.
fn egalites(filtre: &[u8]) -> Vec<(String, String)> {
    let mut r = Vec::new();
    let mut o = filtre;
    while let Some((tag, v, reste)) = lire(o) {
        match tag {
            0xa0 | 0xa1 => r.extend(egalites(v)),
            0xa3 => {
                let (_, a, s) = lire(v).unwrap();
                let (_, val, _) = lire(s).unwrap();
                r.push((
                    String::from_utf8_lossy(a).into_owned(),
                    String::from_utf8_lossy(val).into_owned(),
                ));
            }
            _ => {}
        }
        o = reste;
    }
    r
}

fn servir<S: Read + Write>(mut flux: S, trace: &Mutex<Trace>) {
    let mut tampon = Vec::new();
    let mut morceau = [0u8; 4096];
    loop {
        // Un message LDAP complet ?
        while let Some((0x30, corps, reste)) = lire(&tampon) {
            let corps = corps.to_vec();
            let consomme = tampon.len() - reste.len();
            tampon.drain(..consomme);
            let Some((_, id, op)) = lire(&corps) else { return };
            let Some((tag, op, _)) = lire(op) else { return };
            let reponse = match tag {
                0x60 => {
                    let (_, _, r) = lire(op).unwrap();
                    let (_, dn, r) = lire(r).unwrap();
                    let (_, mdp, _) = lire(r).unwrap();
                    let (dn, mdp) = (String::from_utf8_lossy(dn).into_owned(), mdp.to_vec());
                    let bon = (dn == COMPTE && mdp == MDP_COMPTE.as_bytes())
                        || ((dn == DN_MEMBRE || dn == DN_SPECIAL) && mdp == MDP_MEMBRE.as_bytes());
                    trace.lock().unwrap().liaisons.push(dn);
                    vec![message(id, resultat(0x61, if bon { 0 } else { 49 }))]
                }
                0x63 => {
                    let (_, base, reste) = lire(op).unwrap();
                    let (_, portee, _) = lire(reste).unwrap();
                    let base = String::from_utf8_lossy(base).into_owned();
                    let mut r = op;
                    for _ in 0..6 {
                        r = lire(r).unwrap().2; // base, portée, alias, limites, types
                    }
                    let (t, f, _) = lire(r).unwrap();
                    let filtre = tlv(t, f);
                    let e = egalites(&filtre);
                    let cherche = |nom: &str| {
                        e.iter()
                            .any(|(a, v)| a == "sAMAccountName" && v.eq_ignore_ascii_case(nom))
                    };
                    // Comme un annuaire où le compte de lecture ne peut pas lire memberOf : aucune réponse, aucune erreur.
                    let sans_droit = contient(&filtre, "memberOf");
                    let (membre, special, autre) = (
                        !sans_droit && cherche("membre"),
                        !sans_droit && cherche("special"),
                        !sans_droit && cherche("autre.exemple"),
                    );
                    // Le groupe répond à la règle en chaîne sur member, pour ses seuls membres.
                    let dans_le_groupe = base == GROUPE
                        && contient(&filtre, "member")
                        && contient(&filtre, REGLE_CHAINE)
                        && (contient(&filtre, DN_MEMBRE) || contient(&filtre, DN_SPECIAL));
                    {
                        let mut t = trace.lock().unwrap();
                        t.recherches.push((base, portee.first().copied().unwrap_or(9)));
                        t.filtres.push(filtre);
                    }
                    let mut rep = Vec::new();
                    if dans_le_groupe {
                        rep.push(message(id, entree(GROUPE, &[])));
                    }
                    if membre {
                        rep.push(message(
                            id,
                            entree(
                                DN_MEMBRE,
                                &[
                                    ("sAMAccountName", "Membre"),
                                    ("displayName", "Membre Exemple"),
                                    ("department", "Audit"),
                                ],
                            ),
                        ));
                    }
                    if special {
                        rep.push(message(id, entree(DN_SPECIAL, &[("sAMAccountName", "special")])));
                    }
                    if autre {
                        rep.push(message(
                            id,
                            entree(
                                "CN=Autre,OU=Paie,DC=example,DC=org",
                                &[("sAMAccountName", "autre.exemple")],
                            ),
                        ));
                    }
                    // Comme un vrai contrôleur de domaine : un renvoi à la racine.
                    rep.push(message(
                        id,
                        tlv(0x73, &texte("ldap://renvoi.example.org/DC=example")),
                    ));
                    rep.push(message(id, resultat(0x65, 0)));
                    rep
                }
                _ => return, // fin de liaison
            };
            for m in reponse {
                if flux.write_all(&m).is_err() {
                    return;
                }
            }
            let _ = flux.flush();
        }
        match flux.read(&mut morceau) {
            Ok(0) | Err(_) => return,
            Ok(n) => tampon.extend_from_slice(&morceau[..n]),
        }
    }
}

struct Banc {
    annuaire: Ldap,
    trace: Arc<Mutex<Trace>>,
    _d: tempfile::TempDir,
}

fn conf(d: &tempfile::TempDir, url: &str, plus: &str, mdp_compte: &str) -> Config {
    let f = d.path().join("secret");
    std::fs::write(&f, format!("{mdp_compte}\n")).unwrap();
    Config::lire(&format!(
        "ecoute = \"x\"\nbase = \"b\"\n[annuaire]\nurl = \"{url}\"\nbase = \"DC=example,DC=org\"\n\
         compte = \"{COMPTE}\"\nmot_de_passe_fichier = \"{}\"\ngroupe_autorise = \"{GROUPE}\"\n{plus}",
        f.display()
    ))
    .unwrap()
}

fn banc_en_clair(mdp_compte: &str, plus: &str) -> Banc {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ldap://{}", l.local_addr().unwrap());
    let trace = Arc::new(Mutex::new(Trace::default()));
    let t = Arc::clone(&trace);
    std::thread::spawn(move || {
        for flux in l.incoming().flatten() {
            let t = Arc::clone(&t);
            std::thread::spawn(move || servir(flux, &t));
        }
    });
    let d = tempfile::tempdir().unwrap();
    let c = conf(&d, &url, &format!("ldap_en_clair = true\n{plus}"), mdp_compte);
    Banc {
        annuaire: Ldap::nouveau(c.annuaire.as_ref().unwrap()).unwrap(),
        trace,
        _d: d,
    }
}

fn banc() -> Banc {
    banc_en_clair(MDP_COMPTE, "")
}

#[test]
fn membre_du_groupe_avec_le_bon_mot_de_passe() {
    let b = banc();
    let i = b.annuaire.authentifier("membre", MDP_MEMBRE).unwrap();
    assert_eq!(i.login, "Membre", "identifiant tel qu'écrit dans l'annuaire");
    assert_eq!(i.nom.as_deref(), Some("Membre Exemple"));
    let t = b.trace.lock().unwrap();
    assert_eq!(t.liaisons, vec![COMPTE.to_owned(), DN_MEMBRE.to_owned()]);
    // La personne est cherchée sans memberOf, puis le groupe seul est interrogé, avec la règle des groupes imbriqués.
    assert!(!contient(&t.filtres[0], "memberOf"));
    assert_eq!(t.recherches[1], (GROUPE.to_owned(), 0));
    assert!(contient(&t.filtres[1], DN_MEMBRE));
    assert!(contient(&t.filtres[1], REGLE_CHAINE));
    assert!(!contient(&t.filtres[1], "memberOf"));
}

#[test]
fn dn_avec_caracteres_speciaux_echappe_dans_le_controle_du_groupe() {
    let b = banc();
    assert!(b.annuaire.authentifier("special", MDP_MEMBRE).is_ok());
    // L'étoile et les parenthèses sont arrivées comme un littéral, pas comme un filtre.
    assert!(contient(&b.trace.lock().unwrap().filtres[1], DN_SPECIAL));
}

#[test]
fn identifiant_avec_domaine() {
    let b = banc();
    assert!(b.annuaire.authentifier(r"EXEMPLE\membre", MDP_MEMBRE).is_ok());
    assert!(b.annuaire.authentifier("membre@example.org", MDP_MEMBRE).is_ok());
}

#[test]
fn mauvais_mot_de_passe() {
    let b = banc();
    assert_eq!(b.annuaire.authentifier("membre", "faux"), Err(ErreurAuth::Refus));
}

#[test]
fn compte_hors_groupe_refuse_sans_essayer_son_mot_de_passe() {
    let b = banc();
    assert_eq!(
        b.annuaire.authentifier("autre.exemple", MDP_MEMBRE),
        Err(ErreurAuth::Refus)
    );
    // Seul le compte de lecture s'est lié : aucun échec compté sur le compte de la personne.
    assert_eq!(b.trace.lock().unwrap().liaisons, vec![COMPTE.to_owned()]);
}

#[test]
fn mot_de_passe_vide_refuse_sans_aucun_echange() {
    let b = banc();
    assert_eq!(b.annuaire.authentifier("membre", ""), Err(ErreurAuth::Refus));
    assert_eq!(b.annuaire.authentifier("", MDP_MEMBRE), Err(ErreurAuth::Refus));
    assert!(b.trace.lock().unwrap().liaisons.is_empty());
}

#[test]
fn injection_dans_l_identifiant_neutralisee() {
    let b = banc();
    assert_eq!(
        b.annuaire.authentifier("*)(sAMAccountName=membre", MDP_MEMBRE),
        Err(ErreurAuth::Refus)
    );
    let t = b.trace.lock().unwrap();
    // Seul le compte de lecture s'est lié, et la valeur est restée UN littéral : une seule égalité sur l'identifiant,
    // caractères spéciaux compris.
    assert_eq!(t.liaisons, vec![COMPTE.to_owned()]);
    let sur_l_identifiant: Vec<_> = egalites(&t.filtres[0])
        .into_iter()
        .filter(|(a, _)| a == "sAMAccountName")
        .collect();
    assert_eq!(
        sur_l_identifiant,
        vec![("sAMAccountName".to_owned(), "*)(sAMAccountName=membre".to_owned())]
    );
}

#[test]
fn compte_de_lecture_refuse_ou_annuaire_coupe() {
    let b = banc_en_clair("mot-de-passe-perime", "");
    match b.annuaire.authentifier("membre", MDP_MEMBRE) {
        Err(ErreurAuth::Indisponible(e)) => {
            assert!(!e.contains("membre") && !e.contains("mot-de-passe"), "{e}");
        }
        autre => panic!("attendu Indisponible, reçu {autre:?}"),
    }

    // Port fermé.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ldap://{}", l.local_addr().unwrap());
    drop(l);
    let d = tempfile::tempdir().unwrap();
    let c = conf(&d, &url, "ldap_en_clair = true\n", MDP_COMPTE);
    let a = Ldap::nouveau(c.annuaire.as_ref().unwrap()).unwrap();
    assert!(matches!(
        a.authentifier("membre", MDP_MEMBRE),
        Err(ErreurAuth::Indisponible(_))
    ));
    assert!(a.fiches(&["membre".into()]).is_err());
}

#[test]
fn fiches_nominatives_seulement() {
    // Casse de la configuration différente de celle rendue par l'annuaire : même attribut.
    let b = banc_en_clair(
        MDP_COMPTE,
        "attribut_cabinet = \"dn:ou\"\nattribut_service = \"DEPARTMENT\"\n",
    );
    let f = b
        .annuaire
        .fiches(&["membre".into(), "autre.exemple".into(), "absent".into()])
        .unwrap();
    assert_eq!(f.len(), 2);
    assert_eq!(f[0].service.as_deref(), Some("Audit"));
    assert_eq!(f[0].cabinet.as_deref(), Some("Audit"));
    assert_eq!(f[1].cabinet.as_deref(), Some("Paie"));
    assert_eq!(f[1].nom, None);
    // Recherche par identifiants, pas de lecture de tout l'annuaire.
    let t = b.trace.lock().unwrap();
    assert_eq!(t.filtres.len(), 1);
    assert!(
        egalites(&t.filtres[0])
            .iter()
            .any(|(a, v)| a == "sAMAccountName" && v == "absent")
    );
}

#[test]
fn mot_de_passe_du_compte_introuvable_ou_vide() {
    let d = tempfile::tempdir().unwrap();
    let mut c = conf(&d, "ldaps://x", "", MDP_COMPTE).annuaire.unwrap();
    c.mot_de_passe_fichier = PathBuf::from("/nexiste/pas");
    assert!(Ldap::nouveau(&c).is_err());
    let vide = d.path().join("vide");
    std::fs::write(&vide, "\n").unwrap();
    c.mot_de_passe_fichier = vide;
    assert!(Ldap::nouveau(&c).is_err());
}

// --- Le vrai programme, de bout en bout ---

/// Arrête le serveur à la fin de l'essai, même en cas d'échec.
struct Programme(std::process::Child);
impl Drop for Programme {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Requête HTTP/1.1 minimale : rend (ligne d'état et en-têtes, corps).
fn http(adresse: &str, requete: &str) -> Option<(String, String)> {
    let mut c = std::net::TcpStream::connect(adresse).ok()?;
    c.write_all(requete.as_bytes()).ok()?;
    let mut o = Vec::new();
    c.read_to_end(&mut o).ok()?;
    let t = String::from_utf8_lossy(&o).into_owned();
    let (e, corps) = t.split_once("\r\n\r\n")?;
    Some((e.to_owned(), corps.to_owned()))
}

#[test]
fn programme_complet_connexion_fiche_et_journal() {
    use tirage_modele::{Evenement, ModeCouleur, RectoVerso, Statut, Travail};

    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ldap://{}", l.local_addr().unwrap());
    let trace = Arc::new(Mutex::new(Trace::default()));
    let t = Arc::clone(&trace);
    std::thread::spawn(move || {
        for flux in l.incoming().flatten() {
            let t = Arc::clone(&t);
            std::thread::spawn(move || servir(flux, &t));
        }
    });

    let d = tempfile::tempdir().unwrap();
    let base = d.path().join("tirage.sqlite");
    // Un travail déjà reçu, d'une personne pas encore rattachée.
    tirage_serveur::base::Base::ouvrir(&base)
        .unwrap()
        .enregistrer(
            &Evenement::Travail(Travail {
                collecteur: "impression-1".into(),
                numero: 1,
                file: "COPIEUR-EXEMPLE-1".into(),
                pilote: None,
                utilisateur: "membre".into(),
                poste: None,
                envoye_ms: 1_767_225_600_000,
                imprime_ms: None,
                pages: 1,
                copies: 1,
                feuilles: None,
                couleur: ModeCouleur::Couleur,
                recto_verso: RectoVerso::Recto,
                format: None,
                orientation: None,
                resolution_ppp: None,
                taille_octets: None,
                type_donnees: None,
                statut: Statut::Imprime,
                nom_document: Some("document-fictif.pdf".into()),
                application: None,
            }),
            0,
        )
        .unwrap();
    let secret = d.path().join("secret");
    std::fs::write(&secret, format!("{MDP_COMPTE}\n")).unwrap();
    // Port libre pour le programme.
    let p = TcpListener::bind("127.0.0.1:0").unwrap();
    let adresse = p.local_addr().unwrap().to_string();
    drop(p);
    let config = d.path().join("serveur.toml");
    std::fs::write(
        &config,
        format!(
            "ecoute = \"{adresse}\"\nbase = \"{}\"\n[annuaire]\nurl = \"{url}\"\nldap_en_clair = true\n\
             base = \"DC=example,DC=org\"\ncompte = \"{COMPTE}\"\nmot_de_passe_fichier = \"{}\"\n\
             groupe_autorise = \"{GROUPE}\"\n[interface]\nhttp_en_clair = true\n",
            base.display(),
            secret.display()
        ),
    )
    .unwrap();
    let _programme = Programme(
        std::process::Command::new(env!("CARGO_BIN_EXE_tirage-serveur"))
            .arg("--config")
            .arg(&config)
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let attendre = |mut essai: Box<dyn FnMut() -> bool>| {
        for _ in 0..100 {
            if essai() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    };
    let a = adresse.clone();
    assert!(attendre(Box::new(move || {
        http(&a, "GET /sante HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").is_some()
    })));

    // Page de données sans session : renvoyée à la connexion.
    let (e, _) = http(
        &adresse,
        "GET /journal HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    assert!(e.starts_with("HTTP/1.1 303"), "{e}");

    let poster = |identifiant: &str, mdp: &str| {
        let corps = format!("identifiant={identifiant}&mot_de_passe={mdp}");
        http(
            &adresse,
            &format!(
                "POST /connexion HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
                 Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{corps}",
                corps.len()
            ),
        )
        .unwrap()
    };
    // Compte hors du groupe : refus de bout en bout.
    let (e, _) = poster("autre.exemple", MDP_MEMBRE);
    assert!(e.starts_with("HTTP/1.1 401"), "{e}");
    // Membre du groupe : session.
    let (e, _) = poster("membre", MDP_MEMBRE);
    assert!(e.starts_with("HTTP/1.1 303"), "{e}");
    let cookie = e
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("set-cookie:"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.split(';').next().unwrap().trim().to_owned())
        .unwrap();
    let voir = |chemin: &str| {
        http(
            &adresse,
            &format!("GET {chemin} HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"),
        )
        .unwrap()
    };

    // La fiche arrive de l'annuaire au premier passage du fil de relecture.
    let mut detail = String::new();
    for _ in 0..100 {
        detail = voir("/utilisateurs/membre").1;
        if detail.contains("service : Audit") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(detail.contains("service : Audit"), "{detail}");
    assert!(detail.contains("document-fictif.pdf"));

    let (_, j) = voir("/journal");
    assert!(j.contains("Fiche d&#39;une personne") && j.contains("Connexion refusée"));
}

// --- LDAPS ---

/// Serveur TLS au certificat fictif ; rend son URL et le PEM du certificat.
fn serveur_tls(trace: Arc<Mutex<Trace>>) -> (String, String) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let pem = cert.cert.pem();
    let chaine = vec![cert.cert.der().clone()];
    let cle = rustls::pki_types::PrivateKeyDer::try_from(cert.signing_key.serialize_der()).unwrap();
    let conf = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chaine, cle)
            .unwrap(),
    );
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ldaps://localhost:{}", l.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for flux in l.incoming().flatten() {
            let (conf, t) = (Arc::clone(&conf), Arc::clone(&trace));
            std::thread::spawn(move || {
                let Ok(s) = rustls::ServerConnection::new(conf) else {
                    return;
                };
                servir(rustls::StreamOwned::new(s, flux), &t);
            });
        }
    });
    (url, pem)
}

#[test]
fn ldaps_avec_la_bonne_autorite() {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (url, pem) = serveur_tls(Arc::clone(&trace));
    let d = tempfile::tempdir().unwrap();
    let ca = d.path().join("ca.pem");
    std::fs::write(&ca, pem).unwrap();
    let c = conf(&d, &url, &format!("ca = \"{}\"\n", ca.display()), MDP_COMPTE);
    let a = Ldap::nouveau(c.annuaire.as_ref().unwrap()).unwrap();
    assert_eq!(a.authentifier("membre", MDP_MEMBRE).unwrap().login, "Membre");
    assert_eq!(trace.lock().unwrap().liaisons.len(), 2);
}

#[test]
fn ldaps_refuse_un_certificat_inconnu_avant_tout_envoi() {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (url, _) = serveur_tls(Arc::clone(&trace));
    let d = tempfile::tempdir().unwrap();

    // Une autre autorité…
    let autre = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let ca = d.path().join("autre.pem");
    std::fs::write(&ca, autre.cert.pem()).unwrap();
    let c = conf(&d, &url, &format!("ca = \"{}\"\n", ca.display()), MDP_COMPTE);
    let a = Ldap::nouveau(c.annuaire.as_ref().unwrap()).unwrap();
    assert!(matches!(
        a.authentifier("membre", MDP_MEMBRE),
        Err(ErreurAuth::Indisponible(_))
    ));

    // … ou le magasin du système : même refus, et pas de plantage.
    let c = conf(&d, &url, "", MDP_COMPTE);
    let a = Ldap::nouveau(c.annuaire.as_ref().unwrap()).unwrap();
    assert!(matches!(
        a.authentifier("membre", MDP_MEMBRE),
        Err(ErreurAuth::Indisponible(_))
    ));

    // Aucun identifiant n'a atteint le serveur.
    assert!(trace.lock().unwrap().liaisons.is_empty());
}
