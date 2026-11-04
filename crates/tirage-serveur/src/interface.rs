//! Interface web : connexion réservée au groupe autorisé, détail par utilisateur, journal des consultations.
//!
//! - Toute page de données passe par `exiger_session`, posé sur le routeur : une route ajoutée à `protegees` ne peut
//!   pas l'oublier.
//! - La consultation est écrite au journal **avant** la lecture des données : si le journal est indisponible, la
//!   page n'est pas servie.
//! - Tout texte venu des travaux ou de l'annuaire est échappé (un titre de document peut contenir du HTML).
//! - Cookie `SameSite=Strict` : un autre site ne peut pas déclencher d'action d'une session ouverte.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{DefaultBodyLimit, Extension, Form, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use tirage_modele::iso_utc;

use crate::annuaire::{Annuaire, ErreurAuth, identifiant_nu};
use crate::base::Base;
use crate::config::ConfigInterface;
use crate::session::{Limiteur, Session, Sessions};

const COOKIE: &str = "tirage_session";
const CORPS_MAX: usize = 16 * 1024;
const LONGUEUR_MAX: usize = 1024;
const LIGNES_DETAIL: u32 = 500;
const LIGNES_JOURNAL: u32 = 500;
/// Lecteur consigné pour un essai refusé dont l'identifiant n'est pas connu : ce qui a été saisi n'est pas gardé
/// (on saisit parfois son mot de passe dans le mauvais champ).
const LECTEUR_INCONNU: &str = "(inconnu)";

const FEUILLE_DE_STYLE: &str = "body{font-family:system-ui,sans-serif;margin:2rem;color:#1a1a1a}\
table{border-collapse:collapse;font-size:.9rem}th,td{border:1px solid #ccc;padding:.25rem .5rem;text-align:left}\
th{background:#f0f0f0}.erreur{color:#a00000}label{display:block;margin:.5rem 0}nav form{display:inline}";

pub struct Interface {
    annuaire: Arc<dyn Annuaire>,
    sessions: Sessions,
    limiteur: Limiteur,
    cookie_securise: bool,
}

impl Interface {
    pub fn nouvelle(annuaire: Arc<dyn Annuaire>, c: &ConfigInterface) -> Self {
        Interface {
            annuaire,
            sessions: Sessions::nouvelles(c.session_minutes, c.inactivite_minutes),
            limiteur: Limiteur::default(),
            cookie_securise: !c.http_en_clair,
        }
    }

    fn cookie(&self, valeur: &str, age_secondes: i64) -> String {
        let securite = if self.cookie_securise { "; Secure" } else { "" };
        format!("{COOKIE}={valeur}; Path=/; HttpOnly; SameSite=Strict; Max-Age={age_secondes}{securite}")
    }
}

#[derive(Clone)]
pub struct Contexte {
    pub interface: Arc<Interface>,
    pub base: Arc<Mutex<Base>>,
    pub horloge: fn() -> i64,
}

/// Routes de l'interface, avec leur propre état.
pub fn routes<S: Clone + Send + Sync + 'static>(c: Contexte) -> Router<S> {
    let protegees = Router::new()
        .route("/", get(accueil))
        .route("/utilisateurs/{utilisateur}", get(detail))
        .route("/journal", get(journal))
        .route("/deconnexion", post(deconnexion))
        .route_layer(middleware::from_fn_with_state(c.clone(), exiger_session));
    Router::new()
        .route("/connexion", get(page_connexion).post(connecter))
        .route("/style.css", get(style))
        .merge(protegees)
        .layer(DefaultBodyLimit::max(CORPS_MAX))
        .layer(middleware::map_response(entetes_de_securite))
        .with_state(c)
}

async fn entetes_de_securite(mut r: Response) -> Response {
    let e = r.headers_mut();
    for (nom, valeur) in [
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; style-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        ),
        (header::CACHE_CONTROL, "no-store"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ] {
        e.insert(nom, HeaderValue::from_static(valeur));
    }
    r
}

async fn style() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        FEUILLE_DE_STYLE,
    )
        .into_response()
}

fn jeton_du_cookie(entetes: &HeaderMap) -> Option<&str> {
    entetes
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

async fn exiger_session(State(c): State<Contexte>, mut req: Request, suite: Next) -> Response {
    let session = jeton_du_cookie(req.headers()).and_then(|j| c.interface.sessions.lire(j, (c.horloge)()));
    match session {
        Some(s) => {
            req.extensions_mut().insert(s);
            suite.run(req).await
        }
        None => Redirect::to("/connexion").into_response(),
    }
}

/// Exécute `f` avec la base, hors du fil asynchrone.
async fn avec_base<T: Send + 'static>(
    base: &Arc<Mutex<Base>>,
    f: impl FnOnce(&Base) -> rusqlite::Result<T> + Send + 'static,
) -> Result<T, Box<Response>> {
    let base = Arc::clone(base);
    let r = tokio::task::spawn_blocking(move || match base.lock() {
        Ok(b) => f(&b).map_err(|e| e.to_string()),
        Err(_) => Err("verrou de la base".to_owned()),
    })
    .await;
    match r {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            eprintln!("erreur de la base : {e}");
            Err(Box::new(indisponible()))
        }
        Err(_) => Err(Box::new(indisponible())),
    }
}

fn indisponible() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Html(page(
            "Indisponible",
            "<p>Service indisponible, réessayer plus tard.</p>",
            None,
        )),
    )
        .into_response()
}

pub fn echapper(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => r.push_str("&amp;"),
            '<' => r.push_str("&lt;"),
            '>' => r.push_str("&gt;"),
            '"' => r.push_str("&quot;"),
            '\'' => r.push_str("&#39;"),
            _ => r.push(c),
        }
    }
    r
}

/// Encodage d'un identifiant dans un chemin d'URL.
fn encoder_chemin(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    for o in s.bytes() {
        if o.is_ascii_alphanumeric() || matches!(o, b'-' | b'.' | b'_' | b'~') {
            r.push(char::from(o));
        } else {
            let _ = write!(r, "%{o:02X}");
        }
    }
    r
}

fn page(titre: &str, corps: &str, session: Option<&Session>) -> String {
    let nav = session.map_or(String::new(), |s| {
        format!(
            "<nav><a href=\"/\">Accueil</a> · <a href=\"/journal\">Journal des consultations</a> · {} \
             <form method=\"post\" action=\"/deconnexion\"><button>Se déconnecter</button></form></nav>",
            echapper(s.nom.as_deref().unwrap_or(&s.login))
        )
    });
    format!(
        "<!doctype html><html lang=\"fr\"><head><meta charset=\"utf-8\"><title>{t} — Tirage</title>\
         <link rel=\"stylesheet\" href=\"/style.css\"></head><body>{nav}<h1>{t}</h1>{corps}</body></html>",
        t = echapper(titre)
    )
}

fn formulaire(message: Option<&str>) -> String {
    let m = message.map_or(String::new(), |m| {
        format!("<p class=\"erreur\">{}</p>", echapper(m))
    });
    page(
        "Connexion",
        &format!(
            "{m}<form method=\"post\" action=\"/connexion\">\
             <label>Identifiant <input name=\"identifiant\" autocomplete=\"username\" required></label>\
             <label>Mot de passe <input name=\"mot_de_passe\" type=\"password\" autocomplete=\"current-password\" \
             required></label><button>Se connecter</button></form>"
        ),
        None,
    )
}

async fn page_connexion() -> Html<String> {
    Html(formulaire(None))
}

/// Pas de `Debug` : le mot de passe ne doit pas pouvoir finir dans un journal.
#[derive(Deserialize)]
struct Connexion {
    identifiant: String,
    mot_de_passe: String,
}

async fn connecter(State(c): State<Contexte>, Form(f): Form<Connexion>) -> Response {
    const MESSAGE_REFUS: &str = "Identifiant ou mot de passe incorrect, ou accès non autorisé.";
    let refus = |code, message: &str| (code, Html(formulaire(Some(message)))).into_response();
    if f.identifiant.len() > LONGUEUR_MAX || f.mot_de_passe.len() > LONGUEUR_MAX {
        return refus(StatusCode::UNAUTHORIZED, MESSAGE_REFUS);
    }
    let cle = identifiant_nu(&f.identifiant).to_owned();
    let i = &c.interface;
    if !i.limiteur.reserver(&cle, (c.horloge)()) {
        return refus(
            StatusCode::TOO_MANY_REQUESTS,
            "Trop d'essais pour cet identifiant : réessayer dans 15 minutes.",
        );
    }
    let annuaire = Arc::clone(&i.annuaire);
    let (login, mdp) = (f.identifiant, f.mot_de_passe);
    let r = tokio::task::spawn_blocking(move || annuaire.authentifier(&login, &mdp)).await;
    let maintenant = (c.horloge)();
    match r {
        Ok(Ok(identite)) => {
            let login = identite.login.clone();
            if let Err(rep) = avec_base(&c.base, move |b| {
                b.consigner(maintenant, &login, "connexion", None)
            })
            .await
            {
                return *rep;
            }
            i.limiteur.effacer(&cle);
            let Ok(jeton) = i
                .sessions
                .ouvrir(&identite.login, identite.nom.as_deref(), maintenant)
            else {
                return indisponible();
            };
            let cookie = i.cookie(&jeton, i.sessions.duree_secondes());
            ([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response()
        }
        Ok(Err(ErreurAuth::Refus)) => {
            let cible = cle.clone();
            let consigne = avec_base(&c.base, move |b| {
                let lecteur = if b.personne(&cible)?.is_some() {
                    cible.as_str()
                } else {
                    LECTEUR_INCONNU
                };
                b.consigner(maintenant, lecteur, "connexion_refusee", None)
            })
            .await;
            if let Err(rep) = consigne {
                return *rep;
            }
            refus(StatusCode::UNAUTHORIZED, MESSAGE_REFUS)
        }
        Ok(Err(ErreurAuth::Indisponible(e))) => {
            i.limiteur.rendre(&cle);
            eprintln!("annuaire : {e}");
            refus(
                StatusCode::SERVICE_UNAVAILABLE,
                "Annuaire injoignable, réessayer plus tard.",
            )
        }
        Err(_) => indisponible(),
    }
}

async fn deconnexion(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    entetes: HeaderMap,
) -> Response {
    let maintenant = (c.horloge)();
    // Fermée même si le journal est indisponible : se déconnecter ne doit jamais échouer.
    let _ = avec_base(&c.base, move |b| {
        b.consigner(maintenant, &s.login, "deconnexion", None)
    })
    .await;
    if let Some(j) = jeton_du_cookie(&entetes) {
        c.interface.sessions.fermer(j);
    }
    (
        [(header::SET_COOKIE, c.interface.cookie("", 0))],
        Redirect::to("/connexion"),
    )
        .into_response()
}

async fn accueil(Extension(s): Extension<Session>) -> Html<String> {
    Html(page(
        "Tirage",
        "<p>Le détail d'un utilisateur s'ouvre par <code>/utilisateurs/IDENTIFIANT</code>. \
         Chaque consultation est inscrite au journal.</p>",
        Some(&s),
    ))
}

fn cellule(s: &mut String, v: &str) {
    let _ = write!(s, "<td>{}</td>", echapper(v));
}

async fn detail(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    Path(utilisateur): Path<String>,
) -> Response {
    let maintenant = (c.horloge)();
    let lecteur = s.login.clone();
    let cible = utilisateur.clone();
    let lu = avec_base(&c.base, move |b| {
        // Journal d'abord : sans trace, pas de données.
        b.consigner(maintenant, &lecteur, "detail_utilisateur", Some(&cible))?;
        Ok((b.personne(&cible)?, b.travaux_de(&cible, LIGNES_DETAIL)?))
    })
    .await;
    let (personne, travaux) = match lu {
        Ok(v) => v,
        Err(rep) => return *rep,
    };
    let mut h = String::new();
    if let Some(p) = &personne {
        let _ = write!(
            h,
            "<p>{} — service : {} — cabinet : {}{}</p>",
            echapper(p.nom.as_deref().unwrap_or("nom inconnu")),
            echapper(p.service.as_deref().unwrap_or("inconnu")),
            echapper(p.cabinet.as_deref().unwrap_or("inconnu")),
            if p.trouve { "" } else { " — absent de l'annuaire" },
        );
    }
    h.push_str(
        "<table><tr><th>Envoyé (UTC)</th><th>File</th><th>Poste</th><th>Pages</th><th>Copies</th><th>Feuilles</th>\
         <th>Couleur</th><th>Recto-verso</th><th>Format</th><th>Statut</th><th>Document</th><th>Application</th></tr>",
    );
    for t in &travaux {
        h.push_str("<tr>");
        cellule(&mut h, &iso_utc(t.envoye_ms));
        cellule(&mut h, &t.file);
        cellule(&mut h, t.poste.as_deref().unwrap_or(""));
        cellule(&mut h, &t.pages.to_string());
        cellule(&mut h, &t.copies.to_string());
        cellule(&mut h, &t.feuilles.to_string());
        cellule(&mut h, &t.couleur);
        cellule(&mut h, &t.recto_verso);
        cellule(&mut h, t.format.as_deref().unwrap_or(""));
        cellule(&mut h, &t.statut);
        cellule(&mut h, t.nom_document.as_deref().unwrap_or(""));
        cellule(&mut h, t.application.as_deref().unwrap_or(""));
        h.push_str("</tr>");
    }
    h.push_str("</table>");
    Html(page(&format!("Détail : {utilisateur}"), &h, Some(&s))).into_response()
}

async fn journal(State(c): State<Contexte>, Extension(s): Extension<Session>) -> Response {
    let maintenant = (c.horloge)();
    let lecteur = s.login.clone();
    let lignes = avec_base(&c.base, move |b| {
        b.consigner(maintenant, &lecteur, "journal", None)?;
        b.consultations(LIGNES_JOURNAL)
    })
    .await;
    let lignes = match lignes {
        Ok(l) => l,
        Err(rep) => return *rep,
    };
    let mut h = String::from("<table><tr><th>Quand (UTC)</th><th>Qui</th><th>Action</th><th>Sur</th></tr>");
    for l in &lignes {
        h.push_str("<tr>");
        cellule(&mut h, &iso_utc(l.quand_ms));
        cellule(&mut h, &l.lecteur);
        cellule(&mut h, &l.action);
        match &l.cible {
            Some(u) => {
                let _ = write!(
                    h,
                    "<td><a href=\"/utilisateurs/{}\">{}</a></td>",
                    encoder_chemin(u),
                    echapper(u)
                );
            }
            None => h.push_str("<td></td>"),
        }
        h.push_str("</tr>");
    }
    h.push_str("</table>");
    Html(page("Journal des consultations", &h, Some(&s))).into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use axum::body::Body;
    use http_body_util::BodyExt;
    use tirage_modele::Evenement;
    use tower::ServiceExt;

    use super::*;
    use crate::annuaire::{Fiche, Identite};
    use crate::base::tests::travail;

    const MDP: &str = "mot-de-passe-fictif";

    /// `lecteur` est dans le groupe ; `hors.groupe` a le bon mot de passe mais n'y est pas : refus, comme l'annuaire.
    struct FauxAnnuaire {
        appels: AtomicU32,
        en_panne: bool,
    }
    impl Annuaire for FauxAnnuaire {
        fn authentifier(&self, login: &str, mdp: &str) -> Result<Identite, ErreurAuth> {
            self.appels.fetch_add(1, Ordering::Relaxed);
            if self.en_panne {
                return Err(ErreurAuth::Indisponible("panne".into()));
            }
            if login == "lecteur" && mdp == MDP {
                return Ok(Identite {
                    login: "lecteur".into(),
                    nom: Some("Lecteur <Exemple>".into()),
                });
            }
            Err(ErreurAuth::Refus)
        }
        fn fiches(&self, _: &[String]) -> Result<Vec<Fiche>, String> {
            Ok(vec![])
        }
    }

    struct Banc {
        routeur: Router,
        base: Arc<Mutex<Base>>,
        annuaire: Arc<FauxAnnuaire>,
    }

    fn banc_avec(en_panne: bool, http_en_clair: bool) -> Banc {
        let annuaire = Arc::new(FauxAnnuaire {
            appels: AtomicU32::new(0),
            en_panne,
        });
        let base = Arc::new(Mutex::new(Base::en_memoire().unwrap()));
        let conf = ConfigInterface {
            http_en_clair,
            ..ConfigInterface::default()
        };
        let a: Arc<dyn Annuaire> = annuaire.clone();
        let c = Contexte {
            interface: Arc::new(Interface::nouvelle(a, &conf)),
            base: Arc::clone(&base),
            horloge: || 1000,
        };
        Banc {
            routeur: routes::<()>(c).with_state(()),
            base,
            annuaire,
        }
    }

    fn banc() -> Banc {
        banc_avec(false, false)
    }

    struct Rep {
        code: StatusCode,
        entetes: HeaderMap,
        texte: String,
    }

    impl Banc {
        async fn req(&self, r: axum::http::Request<Body>) -> Rep {
            let rep = self.routeur.clone().oneshot(r).await.unwrap();
            let (code, entetes) = (rep.status(), rep.headers().clone());
            let o = rep.into_body().collect().await.unwrap().to_bytes();
            Rep {
                code,
                entetes,
                texte: String::from_utf8_lossy(&o).into_owned(),
            }
        }

        async fn get(&self, chemin: &str, cookie: Option<&str>) -> Rep {
            let mut r = axum::http::Request::get(chemin);
            if let Some(c) = cookie {
                r = r.header(header::COOKIE, c);
            }
            self.req(r.body(Body::empty()).unwrap()).await
        }

        async fn poster_connexion(&self, identifiant: &str, mdp: &str) -> Rep {
            let corps = format!("identifiant={identifiant}&mot_de_passe={mdp}");
            self.req(
                axum::http::Request::post("/connexion")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(corps))
                    .unwrap(),
            )
            .await
        }

        /// Connecte `lecteur` ; rend l'en-tête Cookie à renvoyer.
        async fn connecter(&self) -> String {
            let r = self.poster_connexion("lecteur", MDP).await;
            assert_eq!(r.code, StatusCode::SEE_OTHER, "{}", r.texte);
            let c = r.entetes.get(header::SET_COOKIE).unwrap().to_str().unwrap();
            c.split(';').next().unwrap().to_owned()
        }

        fn journal(&self) -> Vec<(String, String, Option<String>)> {
            let b = self.base.lock().unwrap();
            b.consultations(100)
                .unwrap()
                .into_iter()
                .map(|l| (l.lecteur, l.action, l.cible))
                .collect()
        }
    }

    #[tokio::test]
    async fn sans_session_tout_renvoie_a_la_connexion() {
        let b = banc();
        for chemin in ["/", "/utilisateurs/a.exemple", "/journal"] {
            let r = b.get(chemin, None).await;
            assert_eq!(r.code, StatusCode::SEE_OTHER, "{chemin}");
            assert_eq!(r.entetes.get(header::LOCATION).unwrap(), "/connexion");
            // Un jeton inventé ne vaut pas mieux.
            let r = b.get(chemin, Some("tirage_session=invente")).await;
            assert_eq!(r.code, StatusCode::SEE_OTHER, "{chemin}");
        }
        assert!(b.journal().is_empty());
    }

    #[tokio::test]
    async fn compte_hors_groupe_et_mauvais_mot_de_passe_indiscernables() {
        let b = banc();
        let hors = b.poster_connexion("hors.groupe", MDP).await;
        let faux = b.poster_connexion("lecteur", "faux").await;
        assert_eq!(hors.code, StatusCode::UNAUTHORIZED);
        assert_eq!(faux.code, StatusCode::UNAUTHORIZED);
        assert_eq!(hors.texte, faux.texte);
        assert!(hors.entetes.get(header::SET_COOKIE).is_none());
        // Ni le mot de passe ni l'identifiant saisi ne sont gardés.
        let j = b.journal();
        assert_eq!(j.len(), 2);
        assert!(
            j.iter()
                .all(|(l, a, _)| l == LECTEUR_INCONNU && a == "connexion_refusee")
        );
    }

    #[tokio::test]
    async fn cookie_protege() {
        let b = banc();
        let r = b.poster_connexion("lecteur", MDP).await;
        let c = r.entetes.get(header::SET_COOKIE).unwrap().to_str().unwrap();
        for attendu in ["HttpOnly", "SameSite=Strict", "Secure", "Path=/"] {
            assert!(c.contains(attendu), "{attendu} : {c}");
        }
        let b = banc_avec(false, true);
        let r = b.poster_connexion("lecteur", MDP).await;
        let c = r.entetes.get(header::SET_COOKIE).unwrap().to_str().unwrap();
        assert!(!c.contains("Secure"));
    }

    #[tokio::test]
    async fn parcours_consultation_journalisee_puis_deconnexion() {
        let b = banc();
        let mut t = travail(1);
        t.nom_document = Some("<script>alert(1)</script>.pdf".into());
        b.base
            .lock()
            .unwrap()
            .enregistrer(&Evenement::Travail(t), 0)
            .unwrap();
        let cookie = b.connecter().await;

        let r = b.get("/", Some(&cookie)).await;
        assert_eq!(r.code, StatusCode::OK);
        assert!(r.texte.contains("Lecteur &lt;Exemple&gt;"));

        // Le détail : consigné, et le titre du document est échappé.
        let r = b.get("/utilisateurs/a.exemple", Some(&cookie)).await;
        assert_eq!(r.code, StatusCode::OK);
        assert!(!r.texte.contains("<script>"));
        assert!(r.texte.contains("&lt;script&gt;"));
        assert_eq!(
            b.journal()[0],
            (
                "lecteur".into(),
                "detail_utilisateur".into(),
                Some("a.exemple".into())
            )
        );

        let r = b.get("/journal", Some(&cookie)).await;
        assert!(r.texte.contains("detail_utilisateur") && r.texte.contains("connexion"));

        // Déconnexion : le même jeton ne vaut plus rien.
        let r = b
            .req(
                axum::http::Request::post("/deconnexion")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(r.code, StatusCode::SEE_OTHER);
        assert!(
            r.entetes
                .get(header::SET_COOKIE)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
        assert_eq!(b.get("/", Some(&cookie)).await.code, StatusCode::SEE_OTHER);
        assert_eq!(b.journal()[0].1, "deconnexion");
    }

    #[tokio::test]
    async fn sans_journal_pas_de_donnees() {
        let b = banc();
        let mut t = travail(1);
        t.nom_document = Some("document-fictif.pdf".into());
        b.base
            .lock()
            .unwrap()
            .enregistrer(&Evenement::Travail(t), 0)
            .unwrap();
        let cookie = b.connecter().await;
        b.base
            .lock()
            .unwrap()
            .connexion()
            .execute_batch("DROP TABLE consultations")
            .unwrap();
        let r = b.get("/utilisateurs/a.exemple", Some(&cookie)).await;
        assert_eq!(r.code, StatusCode::SERVICE_UNAVAILABLE);
        assert!(!r.texte.contains("document-fictif"));
        assert_eq!(
            b.get("/journal", Some(&cookie)).await.code,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn sans_journal_pas_de_connexion() {
        let b = banc();
        b.base
            .lock()
            .unwrap()
            .connexion()
            .execute_batch("DROP TABLE consultations")
            .unwrap();
        let r = b.poster_connexion("lecteur", MDP).await;
        assert_eq!(r.code, StatusCode::SERVICE_UNAVAILABLE);
        assert!(r.entetes.get(header::SET_COOKIE).is_none());
    }

    #[tokio::test]
    async fn trop_d_essais_n_interroge_plus_l_annuaire() {
        let b = banc();
        for _ in 0..5 {
            assert_eq!(
                b.poster_connexion("lecteur", "faux").await.code,
                StatusCode::UNAUTHORIZED
            );
        }
        let r = b.poster_connexion("LECTEUR", MDP).await;
        assert_eq!(r.code, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(b.annuaire.appels.load(Ordering::Relaxed), 5);
    }

    #[tokio::test]
    async fn annuaire_en_panne() {
        let b = banc_avec(true, false);
        for _ in 0..7 {
            let r = b.poster_connexion("lecteur", MDP).await;
            assert_eq!(r.code, StatusCode::SERVICE_UNAVAILABLE);
            assert!(r.entetes.get(header::SET_COOKIE).is_none());
        }
        // Une panne ne compte pas comme un mauvais mot de passe, et ne se consigne pas comme un refus.
        assert_eq!(b.annuaire.appels.load(Ordering::Relaxed), 7);
        assert!(b.journal().is_empty());
    }

    #[tokio::test]
    async fn entetes_de_securite_sur_toutes_les_pages() {
        let b = banc();
        for chemin in ["/connexion", "/style.css", "/"] {
            let r = b.get(chemin, None).await;
            assert_eq!(
                r.entetes.get(header::CACHE_CONTROL).unwrap(),
                "no-store",
                "{chemin}"
            );
            assert!(
                r.entetes.contains_key(header::CONTENT_SECURITY_POLICY),
                "{chemin}"
            );
        }
    }

    #[tokio::test]
    async fn sans_interface_aucune_page() {
        use crate::api::{self, Etat};
        use crate::config::Config;
        let etat = Etat {
            config: Arc::new(Config::lire("ecoute = \"x\"\nbase = \"b\"\n").unwrap()),
            base: Arc::new(Mutex::new(Base::en_memoire().unwrap())),
            horloge: || 0,
            interface: None,
        };
        for chemin in ["/", "/connexion", "/journal", "/utilisateurs/a"] {
            let r = api::routeur(etat.clone())
                .oneshot(axum::http::Request::get(chemin).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{chemin}");
        }
    }

    #[test]
    fn encodage_du_chemin() {
        assert_eq!(encoder_chemin("a.exemple"), "a.exemple");
        assert_eq!(encoder_chemin(r"D\a b/é"), "D%5Ca%20b%2F%C3%A9");
    }

    #[test]
    fn cookie_lu_parmi_d_autres() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("x=1; tirage_session_autre=2; tirage_session=abc"),
        );
        assert_eq!(jeton_du_cookie(&h), Some("abc"));
    }
}
