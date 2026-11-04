//! Bout en bout : la file du vrai collecteur est envoyée au vrai serveur (HTTP local), renvoi compris.

use std::sync::{Arc, Mutex};

use tirage_collecteur::config::Serveur;
use tirage_collecteur::envoi::{self, Bilan, Expediteur, Http};
use tirage_file::FileAttente;
use tirage_modele::{Enveloppe, Evenement, ModeCouleur, RectoVerso, Statut, Travail};
use tirage_serveur::api::{self, Etat};
use tirage_serveur::base::Base;
use tirage_serveur::config::{Config, empreinte};

const JETON: &str = "jeton-essai-bout-en-bout";

fn travail(numero: u32) -> Vec<u8> {
    Enveloppe::nouvelle(Evenement::Travail(Travail {
        collecteur: "impression-1".into(),
        numero,
        file: "COPIEUR-EXEMPLE-1".into(),
        pilote: None,
        utilisateur: "a.exemple".into(),
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
        nom_document: None,
        application: None,
    }))
    .encoder()
}

/// Démarre le serveur sur un port libre ; rend son URL et sa base.
fn serveur() -> (String, Arc<Mutex<Base>>) {
    let config = Config::lire(&format!(
        "ecoute = \"127.0.0.1:0\"\nbase = \"x\"\n[[collecteurs]]\nnom = \"impression-1\"\njeton_sha256 = \"{}\"\n",
        empreinte(JETON)
    ))
    .unwrap();
    let base = Arc::new(Mutex::new(Base::en_memoire().unwrap()));
    let etat = Etat {
        config: Arc::new(config),
        base: Arc::clone(&base),
        horloge: || 0,
        interface: None,
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(l.local_addr().unwrap()).unwrap();
            axum::serve(l, api::routeur(etat)).await.unwrap();
        });
    });
    (format!("http://{}", rx.recv().unwrap()), base)
}

fn expediteur(url: &str, jeton: &str) -> Http {
    Http::nouveau(&Serveur {
        url: url.into(),
        jeton: jeton.into(),
        ca: None,
        http_en_clair: true,
    })
    .unwrap()
}

#[test]
fn la_file_arrive_en_base_sans_doublon() {
    let (url, base) = serveur();
    let d = tempfile::tempdir().unwrap();
    let mut f = FileAttente::ouvrir(d.path(), 1000).unwrap();
    for n in 1..=3 {
        f.deposer(&travail(n)).unwrap();
    }
    f.deposer(br#"{"version":99,"evenement":{}}"#).unwrap();
    let mut http = expediteur(&url, JETON);
    assert_eq!(
        envoi::envoyer_un_lot(&mut f, &mut http, 100).unwrap(),
        Bilan::Envoye {
            acquittes: 3,
            refuses: 1
        }
    );
    assert!(f.lot(10).unwrap().is_empty());
    assert_eq!(base.lock().unwrap().nombre_travaux().unwrap(), 3);

    // Accusé perdu : le collecteur renvoie les mêmes travaux ; ils sont acquittés sans doublon.
    for n in 1..=3 {
        f.deposer(&travail(n)).unwrap();
    }
    assert_eq!(
        envoi::envoyer_un_lot(&mut f, &mut http, 100).unwrap(),
        Bilan::Envoye {
            acquittes: 3,
            refuses: 0
        }
    );
    assert_eq!(base.lock().unwrap().nombre_travaux().unwrap(), 3);
}

#[test]
fn mauvais_jeton_rien_n_est_retire() {
    let (url, base) = serveur();
    let d = tempfile::tempdir().unwrap();
    let mut f = FileAttente::ouvrir(d.path(), 1000).unwrap();
    f.deposer(&travail(1)).unwrap();
    let r = envoi::envoyer_un_lot(&mut f, &mut expediteur(&url, "faux"), 100);
    assert!(matches!(r, Err(envoi::ErreurEnvoi::Refus(401, _))), "{r:?}");
    assert_eq!(f.lot(10).unwrap().len(), 1);
    assert_eq!(base.lock().unwrap().nombre_travaux().unwrap(), 0);
}

#[test]
fn serveur_arrete_le_collecteur_attend() {
    // Port fermé : erreur passagère, la file est intacte.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    drop(l);
    let d = tempfile::tempdir().unwrap();
    let mut f = FileAttente::ouvrir(d.path(), 1000).unwrap();
    f.deposer(&travail(1)).unwrap();
    let mut http = expediteur(&url, JETON);
    assert!(http.poster(b"{}").is_err());
    assert!(matches!(
        envoi::envoyer_un_lot(&mut f, &mut http, 100),
        Err(envoi::ErreurEnvoi::Passagere(_))
    ));
    assert_eq!(f.lot(10).unwrap().len(), 1);
}
