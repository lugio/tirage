//! Interface web : connexion réservée au groupe autorisé, tableaux, palmarès, coûts, export CSV, détail par
//! utilisateur, état des copieurs, journal des consultations.
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
use axum::extract::{DefaultBodyLimit, Extension, Form, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use serde::Deserialize;

use crate::annuaire::{Annuaire, ErreurAuth, identifiant_nu};
use crate::base::Base;
use crate::config::{ConfigCouts, ConfigInterface};
use crate::rapports::{self, Axe, Ligne};
use crate::session::{Limiteur, Session, Sessions};

const COOKIE: &str = "tirage_session";
const CORPS_MAX: usize = 16 * 1024;
const LONGUEUR_MAX: usize = 1024;
const LIGNES_DETAIL: u32 = 500;
const LIGNES_JOURNAL: u32 = 500;
/// Lecteur consigné pour un essai refusé dont l'identifiant n'est pas connu : ce qui a été saisi n'est pas gardé
/// (on saisit parfois son mot de passe dans le mauvais champ).
const LECTEUR_INCONNU: &str = "(inconnu)";

const PALMARES_TAILLE: usize = 10;

const FEUILLE_DE_STYLE: &str = "html{color-scheme:light}\
body{font-family:system-ui,sans-serif;margin:2rem;color:#1a1a1a;background:#fff}\
a{color:#0f5e58}nav{margin-bottom:1rem}nav form{display:inline}\
table{border-collapse:collapse;font-size:.9rem;margin:.5rem 0 1.5rem}\
th,td{border-bottom:1px solid #ddd;padding:.3rem .6rem;text-align:left}th{color:#555;font-weight:600}\
td.n,th.n{text-align:right;font-variant-numeric:tabular-nums}tr.total td{font-weight:600;border-top:2px solid #888}\
.erreur{color:#a00000}.note{color:#555;font-size:.9rem}label{display:block;margin:.5rem 0}\
form.periode label{display:inline;margin-right:1rem}\
.tuiles{display:flex;flex-wrap:wrap;gap:1rem;margin:1rem 0}.tuile{border:1px solid #ddd;border-radius:6px;\
padding:.75rem 1rem;min-width:9rem}.tuile b{display:block;font-size:1.5rem;font-variant-numeric:tabular-nums}\
.tuile span{color:#555;font-size:.85rem}\
.palmares{display:flex;flex-wrap:wrap;gap:2rem}";

pub struct Interface {
    annuaire: Arc<dyn Annuaire>,
    sessions: Sessions,
    limiteur: Limiteur,
    cookie_securise: bool,
    fuseau: TimeZone,
    couts: ConfigCouts,
}

impl Interface {
    pub fn nouvelle(annuaire: Arc<dyn Annuaire>, c: &ConfigInterface, couts: ConfigCouts) -> Self {
        Interface {
            annuaire,
            sessions: Sessions::nouvelles(c.session_minutes, c.inactivite_minutes),
            limiteur: Limiteur::default(),
            cookie_securise: !c.http_en_clair,
            // La configuration a déjà validé le nom du fuseau.
            fuseau: TimeZone::get(&c.fuseau).unwrap_or(TimeZone::UTC),
            couts,
        }
    }

    fn heure(&self, ms: i64) -> String {
        Timestamp::from_millisecond(ms).map_or_else(
            |_| "date invalide".to_owned(),
            |t| {
                t.to_zoned(self.fuseau.clone())
                    .strftime("%d/%m/%Y %H:%M")
                    .to_string()
            },
        )
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
        .route("/palmares", get(palmares))
        .route("/rapports/{axe}", get(rapport))
        .route("/rapports/{axe}/csv", get(export_csv))
        .route("/copieurs", get(copieurs))
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
            "<nav><a href=\"/\">Accueil</a> · <a href=\"/palmares\">Palmarès</a> · <a href=\"/copieurs\">Copieurs</a> · \
             <a href=\"/journal\">Journal des consultations</a> · {} \
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

/// Période demandée : `du` et `au` en AAAA-MM-JJ, bornes incluses, dans le fuseau configuré.
#[derive(Deserialize)]
struct Demande {
    du: Option<String>,
    au: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Periode {
    du: Date,
    au: Date,
    du_ms: i64,
    au_ms: i64,
}

impl Periode {
    /// Par défaut : du premier du mois courant à aujourd'hui.
    fn lire(d: &Demande, maintenant_ms: i64, fuseau: &TimeZone) -> Result<Periode, String> {
        let aujourd_hui = Timestamp::from_millisecond(maintenant_ms)
            .map_err(|_| "horloge invalide".to_owned())?
            .to_zoned(fuseau.clone())
            .date();
        let lire = |v: &Option<String>, defaut: Date| match v.as_deref().map(str::trim) {
            None | Some("") => Ok(defaut),
            Some(t) => t
                .parse::<Date>()
                .map_err(|_| "Date invalide : le format attendu est AAAA-MM-JJ.".to_owned()),
        };
        let du = lire(&d.du, aujourd_hui.first_of_month())?;
        let au = lire(&d.au, aujourd_hui)?;
        if au < du {
            return Err("La fin de la période précède son début.".into());
        }
        let debut = |j: Date| {
            j.to_zoned(fuseau.clone())
                .map(|z| z.timestamp().as_millisecond())
                .map_err(|_| "Date hors limites.".to_owned())
        };
        let lendemain = au
            .checked_add(1.day())
            .map_err(|_| "Date hors limites.".to_owned())?;
        Ok(Periode {
            du,
            au,
            du_ms: debut(du)?,
            au_ms: debut(lendemain)?,
        })
    }

    fn texte(&self) -> String {
        format!("{}..{}", self.du, self.au)
    }

    fn requete(&self) -> String {
        format!("du={}&au={}", self.du, self.au)
    }

    fn formulaire(&self, action: &str) -> String {
        format!(
            "<form class=\"periode\" method=\"get\" action=\"{action}\">\
             <label>Du <input type=\"date\" name=\"du\" value=\"{}\"></label>\
             <label>au <input type=\"date\" name=\"au\" value=\"{}\"></label>\
             <button>Afficher</button></form>",
            self.du, self.au
        )
    }
}

fn demande_invalide(message: &str, s: Option<&Session>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Html(page(
            "Demande invalide",
            &format!("<p class=\"erreur\">{}</p>", echapper(message)),
            s,
        )),
    )
        .into_response()
}

/// Consigne la lecture puis rend les lignes de chaque axe demandé, avec le nombre de travaux non comptés.
async fn lire_rapports(
    c: &Contexte,
    lecteur: &str,
    action: String,
    periode: &Periode,
    axes: Vec<Axe>,
) -> Result<(Vec<Vec<Ligne>>, i64), Box<Response>> {
    let (maintenant, lecteur) = ((c.horloge)(), lecteur.to_owned());
    let (cible, du, au) = (periode.texte(), periode.du_ms, periode.au_ms);
    let interface = Arc::clone(&c.interface);
    avec_base(&c.base, move |b| {
        // Journal d'abord : sans trace, pas de données.
        b.consigner(maintenant, &lecteur, &action, Some(&cible))?;
        let mut lignes = Vec::with_capacity(axes.len());
        for axe in axes {
            lignes.push(rapports::construire(&b.agreger(axe, du, au)?, &interface.couts));
        }
        Ok((lignes, b.non_imprimes(du, au)?))
    })
    .await
}

fn entier(n: i64) -> String {
    let brut = n.unsigned_abs().to_string();
    let mut r = String::new();
    for (i, c) in brut.chars().enumerate() {
        if i > 0 && (brut.len() - i) % 3 == 0 {
            r.push('\u{202f}');
        }
        r.push(c);
    }
    if n < 0 { format!("-{r}") } else { r }
}

fn pourcentage(partie: i64, tout: i64) -> String {
    if tout == 0 {
        return "—".into();
    }
    format!("{} %", (partie * 100 + tout / 2) / tout)
}

fn tuile(valeur: &str, libelle: &str) -> String {
    format!(
        "<div class=\"tuile\"><b>{}</b><span>{}</span></div>",
        echapper(valeur),
        echapper(libelle)
    )
}

fn note_sur_le_calcul(total: &Ligne, non_imprimes: i64, devise: &str) -> String {
    let mut n = String::from("<p class=\"note\">Seuls les travaux imprimés sont comptés.");
    if non_imprimes > 0 {
        let _ = write!(
            n,
            " {} travaux annulés ou en erreur sont exclus.",
            entier(non_imprimes)
        );
    }
    if total.couleur_inconnue > 0 {
        let _ = write!(
            n,
            " {} pages au mode couleur inconnu sont chiffrées au tarif noir.",
            entier(total.couleur_inconnue)
        );
    }
    if total.sans_tarif > 0 {
        let _ = write!(
            n,
            " {} pages sans tarif ne sont pas dans le coût : il est donc incomplet ({}).",
            entier(total.sans_tarif),
            echapper(devise)
        );
    }
    n.push_str("</p>");
    n
}

async fn accueil(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    Query(d): Query<Demande>,
) -> Response {
    let i = &c.interface;
    let periode = match Periode::lire(&d, (c.horloge)(), &i.fuseau) {
        Ok(p) => p,
        Err(m) => return demande_invalide(&m, Some(&s)),
    };
    let (lignes, non_imprimes) =
        match lire_rapports(&c, &s.login, "tableau".into(), &periode, vec![Axe::Copieur]).await {
            Ok(v) => v,
            Err(rep) => return *rep,
        };
    let t = rapports::total(lignes.first().map_or(&[][..], Vec::as_slice));
    let mut h = periode.formulaire("/");
    h.push_str("<div class=\"tuiles\">");
    h.push_str(&tuile(&entier(t.pages), "pages imprimées"));
    h.push_str(&tuile(&entier(t.feuilles), "feuilles (estimées au besoin)"));
    h.push_str(&tuile(&pourcentage(t.couleur, t.pages), "des pages en couleur"));
    h.push_str(&tuile(
        &pourcentage(t.recto_verso, t.pages),
        "des pages en recto-verso",
    ));
    h.push_str(&tuile(
        &format!("{} {}", rapports::montant(t.cout), i.couts.devise),
        "coût",
    ));
    h.push_str("</div>");
    h.push_str(&note_sur_le_calcul(&t, non_imprimes, &i.couts.devise));
    h.push_str("<p>Par ");
    let liens: Vec<String> = Axe::TOUS
        .iter()
        .map(|a| {
            format!(
                "<a href=\"/rapports/{}?{}\">{}</a>",
                a.chemin(),
                periode.requete(),
                a.titre().to_lowercase()
            )
        })
        .collect();
    h.push_str(&liens.join(" · "));
    h.push_str("</p><p class=\"note\">Chaque consultation est inscrite au journal.</p>");
    Html(page("Tirage", &h, Some(&s))).into_response()
}

async fn rapport(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    Path(axe): Path<String>,
    Query(d): Query<Demande>,
) -> Response {
    let Some(axe) = Axe::du_chemin(&axe) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let i = &c.interface;
    let periode = match Periode::lire(&d, (c.horloge)(), &i.fuseau) {
        Ok(p) => p,
        Err(m) => return demande_invalide(&m, Some(&s)),
    };
    let action = format!("rapport_{}", axe.chemin());
    let (lignes, non_imprimes) = match lire_rapports(&c, &s.login, action, &periode, vec![axe]).await {
        Ok(v) => v,
        Err(rep) => return *rep,
    };
    let lignes = lignes.first().map_or(&[][..], Vec::as_slice);
    let t = rapports::total(lignes);
    let mut h = periode.formulaire(&format!("/rapports/{}", axe.chemin()));
    let _ = write!(
        h,
        "<p><a href=\"/rapports/{}/csv?{}\">Exporter en CSV</a></p>",
        axe.chemin(),
        periode.requete()
    );
    h.push_str(&note_sur_le_calcul(&t, non_imprimes, &i.couts.devise));
    let _ = write!(h, "<table><tr><th>{}</th>", echapper(axe.titre()));
    if axe == Axe::Utilisateur {
        h.push_str("<th>Nom</th>");
    }
    let _ = write!(
        h,
        "<th class=\"n\">Travaux</th><th class=\"n\">Pages</th><th class=\"n\">N&amp;B</th><th class=\"n\">Couleur</th>\
         <th class=\"n\">Mode inconnu</th><th class=\"n\">Recto-verso</th><th class=\"n\">Feuilles</th>\
         <th class=\"n\">Coût ({})</th></tr>",
        echapper(&i.couts.devise)
    );
    let chiffres = |h: &mut String, l: &Ligne| {
        for v in [
            l.travaux,
            l.pages,
            l.noir,
            l.couleur,
            l.couleur_inconnue,
            l.recto_verso,
            l.feuilles,
        ] {
            let _ = write!(h, "<td class=\"n\">{}</td>", entier(v));
        }
        let _ = write!(h, "<td class=\"n\">{}</td>", rapports::montant(l.cout));
    };
    for l in lignes {
        h.push_str("<tr>");
        if axe == Axe::Utilisateur {
            let _ = write!(
                h,
                "<td><a href=\"/utilisateurs/{}\">{}</a></td>",
                encoder_chemin(&l.cle),
                echapper(&l.cle)
            );
            cellule(&mut h, l.nom.as_deref().unwrap_or(""));
        } else {
            cellule(&mut h, &l.cle);
        }
        chiffres(&mut h, l);
        h.push_str("</tr>");
    }
    h.push_str("<tr class=\"total\"><td>Total</td>");
    if axe == Axe::Utilisateur {
        h.push_str("<td></td>");
    }
    chiffres(&mut h, &t);
    h.push_str("</tr></table>");
    Html(page(&format!("Par {}", axe.titre().to_lowercase()), &h, Some(&s))).into_response()
}

async fn export_csv(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    Path(axe): Path<String>,
    Query(d): Query<Demande>,
) -> Response {
    let Some(axe) = Axe::du_chemin(&axe) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let i = &c.interface;
    let periode = match Periode::lire(&d, (c.horloge)(), &i.fuseau) {
        Ok(p) => p,
        Err(m) => return demande_invalide(&m, Some(&s)),
    };
    let action = format!("export_{}", axe.chemin());
    let (lignes, _) = match lire_rapports(&c, &s.login, action, &periode, vec![axe]).await {
        Ok(v) => v,
        Err(rep) => return *rep,
    };
    let corps = rapports::csv(
        axe,
        lignes.first().map_or(&[][..], Vec::as_slice),
        &i.couts.devise,
    );
    let nom = format!(
        "attachment; filename=\"tirage-{}-{}-{}.csv\"",
        axe.chemin(),
        periode.du,
        periode.au
    );
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (header::CONTENT_DISPOSITION, nom),
        ],
        corps,
    )
        .into_response()
}

/// Barre horizontale d'une valeur rapportée au maximum de la liste. Le chiffre est dans la colonne voisine.
fn barre(valeur: i64, max: i64, libelle: &str) -> String {
    const LARGEUR: i64 = 180;
    let l = if max > 0 {
        (valeur * LARGEUR / max).max(1)
    } else {
        0
    };
    format!(
        "<svg width=\"{LARGEUR}\" height=\"12\" role=\"img\" aria-label=\"{t}\"><title>{t}</title>\
         <rect width=\"{l}\" height=\"12\" rx=\"4\" fill=\"#0d9488\"/></svg>",
        t = echapper(libelle)
    )
}

fn liste_palmares(
    titre: &str,
    tout: &[Ligne],
    mesure: fn(&Ligne) -> i64,
    valeur: fn(i64) -> String,
) -> String {
    let lignes = rapports::palmares(tout, mesure, PALMARES_TAILLE);
    let mut h = format!("<section><h2>{}</h2>", echapper(titre));
    if lignes.is_empty() {
        h.push_str("<p class=\"note\">Rien sur cette période.</p></section>");
        return h;
    }
    h.push_str("<table>");
    let max = lignes.first().map_or(0, |l| mesure(l));
    for (rang, l) in lignes.iter().enumerate() {
        let nom = l.nom.as_deref().unwrap_or(&l.cle);
        let v = valeur(mesure(l));
        let _ = write!(
            h,
            "<tr><td class=\"n\">{}</td><td>{}</td><td class=\"n\">{}</td><td>{}</td></tr>",
            rang + 1,
            echapper(nom),
            echapper(&v),
            barre(mesure(l), max, &format!("{nom} : {v}"))
        );
    }
    h.push_str("</table></section>");
    h
}

async fn palmares(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    Query(d): Query<Demande>,
) -> Response {
    let periode = match Periode::lire(&d, (c.horloge)(), &c.interface.fuseau) {
        Ok(p) => p,
        Err(m) => return demande_invalide(&m, Some(&s)),
    };
    let axes = vec![Axe::Utilisateur, Axe::Service, Axe::Copieur];
    let lu = match lire_rapports(&c, &s.login, "palmares".into(), &periode, axes).await {
        Ok((l, _)) => l,
        Err(rep) => return *rep,
    };
    let vide = Vec::new();
    let [u, sv, co] = [0, 1, 2].map(|n| lu.get(n).unwrap_or(&vide));
    let mut h = periode.formulaire("/palmares");
    h.push_str("<div class=\"palmares\">");
    h.push_str(&liste_palmares("Utilisateurs : pages", u, |l| l.pages, entier));
    h.push_str(&liste_palmares(
        "Utilisateurs : pages couleur",
        u,
        |l| l.couleur,
        entier,
    ));
    h.push_str(&liste_palmares(
        "Utilisateurs : coût",
        u,
        |l| l.cout,
        rapports::montant,
    ));
    h.push_str(&liste_palmares("Services : pages", sv, |l| l.pages, entier));
    h.push_str(&liste_palmares("Copieurs : pages", co, |l| l.pages, entier));
    h.push_str("</div>");
    Html(page("Palmarès", &h, Some(&s))).into_response()
}

async fn copieurs(State(c): State<Contexte>, Extension(s): Extension<Session>) -> Response {
    let releves = match avec_base(&c.base, Base::derniers_releves).await {
        Ok(r) => r,
        Err(rep) => return *rep,
    };
    let mut h = String::new();
    if releves.is_empty() {
        h.push_str(
            "<p class=\"note\">Aucun relevé reçu : le relevé SNMP des copieurs n'est pas encore actif.</p>",
        );
    } else {
        h.push_str(
            "<table><tr><th>Copieur</th><th>Modèle</th><th>Relevé le</th><th class=\"n\">Compteur total</th>\
             <th class=\"n\">N&amp;B</th><th class=\"n\">Couleur</th><th>État</th><th>Consommables</th><th>Alertes</th></tr>",
        );
        let compteur =
            |v: Option<u64>| v.map_or("—".to_owned(), |n| entier(i64::try_from(n).unwrap_or(i64::MAX)));
        for r in &releves {
            h.push_str("<tr>");
            cellule(&mut h, &r.copieur);
            cellule(&mut h, r.modele.as_deref().unwrap_or(""));
            cellule(&mut h, &c.interface.heure(r.releve_ms));
            for v in [r.compteur_total, r.compteur_noir, r.compteur_couleur] {
                let _ = write!(h, "<td class=\"n\">{}</td>", compteur(v));
            }
            cellule(&mut h, r.etat.as_deref().unwrap_or(""));
            let conso: Vec<String> = r
                .consommables
                .iter()
                .map(|x| match x.pourcentage() {
                    Some(p) => format!("{} : {p} %", x.description),
                    None => x.description.clone(),
                })
                .collect();
            cellule(&mut h, &conso.join(", "));
            cellule(&mut h, &r.alertes.join(", "));
            h.push_str("</tr>");
        }
        h.push_str("</table>");
    }
    Html(page("État des copieurs", &h, Some(&s))).into_response()
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
        "<table><tr><th>Envoyé</th><th>File</th><th>Poste</th><th>Pages</th><th>Copies</th><th>Feuilles</th>\
         <th>Couleur</th><th>Recto-verso</th><th>Format</th><th>Statut</th><th>Document</th><th>Application</th></tr>",
    );
    for t in &travaux {
        h.push_str("<tr>");
        cellule(&mut h, &c.interface.heure(t.envoye_ms));
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
    let mut h = String::from("<table><tr><th>Quand</th><th>Qui</th><th>Action</th><th>Sur</th></tr>");
    for l in &lignes {
        h.push_str("<tr>");
        cellule(&mut h, &c.interface.heure(l.quand_ms));
        cellule(&mut h, &l.lecteur);
        cellule(&mut h, &l.action);
        match &l.cible {
            // Lien seulement vers un détail : les autres lignes portent une période.
            Some(u) if l.action == "detail_utilisateur" => {
                let _ = write!(
                    h,
                    "<td><a href=\"/utilisateurs/{}\">{}</a></td>",
                    encoder_chemin(u),
                    echapper(u)
                );
            }
            Some(autre) => cellule(&mut h, autre),
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
    use tirage_modele::{Evenement, ModeCouleur};
    use tower::ServiceExt;

    use super::*;
    use crate::annuaire::{Fiche, Identite};
    use crate::base::tests::travail;

    const MDP: &str = "mot-de-passe-fictif";
    /// 2026-11-15 12:00 UTC.
    const MAINTENANT: i64 = 1_794_744_000_000;
    /// 2026-11-01 00:30 à Paris, encore le 31/10 en UTC.
    const DEBUT_NOVEMBRE: i64 = 1_793_489_400_000;

    fn couts_fictifs() -> ConfigCouts {
        ConfigCouts {
            devise: "€".into(),
            noir: Some(0.01),
            couleur: Some(0.1),
            files: vec![],
        }
    }

    /// Jeu fictif : trois personnes, deux services, deux copieurs.
    fn charger_jeu(b: &Banc) {
        let mut base = b.base.lock().unwrap();
        let mut ajouter = |n: u32, u: &str, file: &str, couleur, pages| {
            let mut t = travail(n);
            t.utilisateur = u.into();
            t.file = file.into();
            t.couleur = couleur;
            t.pages = pages;
            t.copies = 1;
            t.envoye_ms = DEBUT_NOVEMBRE + i64::from(n);
            t.nom_document = Some("document-confidentiel-fictif.pdf".into());
            base.enregistrer(&Evenement::Travail(t), 0).unwrap();
        };
        ajouter(1, "a.exemple", "COPIEUR-1", ModeCouleur::NoirEtBlanc, 100);
        ajouter(2, "a.exemple", "COPIEUR-2", ModeCouleur::Couleur, 10);
        ajouter(3, "b.exemple", "COPIEUR-1", ModeCouleur::Couleur, 50);
        ajouter(4, "c.exemple", "COPIEUR-1", ModeCouleur::NoirEtBlanc, 1);
        base.personne_trouvee(
            "a.exemple",
            Some("A <b>Exemple</b>"),
            Some("Audit"),
            Some("Est"),
            0,
        )
        .unwrap();
        base.personne_trouvee("b.exemple", Some("B Exemple"), Some("Paie"), Some("Est"), 0)
            .unwrap();
    }

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
            fuseau: "Europe/Paris".into(),
            ..ConfigInterface::default()
        };
        let a: Arc<dyn Annuaire> = annuaire.clone();
        let c = Contexte {
            interface: Arc::new(Interface::nouvelle(a, &conf, couts_fictifs())),
            base: Arc::clone(&base),
            horloge: || MAINTENANT,
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
        for chemin in [
            "/",
            "/palmares",
            "/copieurs",
            "/rapports/service",
            "/rapports/utilisateur/csv",
            "/utilisateurs/a.exemple",
            "/journal",
        ] {
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
        assert_eq!(b.journal()[0].1, "tableau");

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

    const NOVEMBRE: &str = "du=2026-11-01&au=2026-11-30";

    #[tokio::test]
    async fn tableau_et_rapports_sur_le_jeu_fictif() {
        let b = banc();
        charger_jeu(&b);
        let cookie = b.connecter().await;

        // Période par défaut : le mois courant. 161 pages ; 60 en couleur = 37 %.
        let r = b.get("/", Some(&cookie)).await;
        assert!(r.texte.contains("<b>161</b>"), "{}", r.texte);
        assert!(r.texte.contains("<b>37 %</b>"));
        // 101 N&B × 0,01 + 60 couleur × 0,10 = 7,01
        assert!(r.texte.contains("<b>7,01 €</b>"));

        let r = b
            .get(&format!("/rapports/service?{NOVEMBRE}"), Some(&cookie))
            .await;
        assert_eq!(r.code, StatusCode::OK);
        assert!(r.texte.contains("Audit") && r.texte.contains("Paie"));
        assert!(r.texte.contains("Non rattaché"), "personne sans fiche visible");

        let r = b
            .get(&format!("/rapports/utilisateur?{NOVEMBRE}"), Some(&cookie))
            .await;
        assert!(r.texte.contains("href=\"/utilisateurs/a.exemple\""));
        assert!(
            r.texte.contains("A &lt;b&gt;Exemple"),
            "nom de l'annuaire échappé"
        );

        for axe in ["cabinet", "copieur", "format"] {
            let r = b.get(&format!("/rapports/{axe}?{NOVEMBRE}"), Some(&cookie)).await;
            assert_eq!(r.code, StatusCode::OK, "{axe}");
        }
        assert_eq!(
            b.get("/rapports/inconnu", Some(&cookie)).await.code,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            b.get("/rapports/inconnu/csv", Some(&cookie)).await.code,
            StatusCode::NOT_FOUND
        );

        // Chaque lecture est au journal, avec sa période.
        let j = b.journal();
        assert!(j.iter().any(|(l, a, c)| l == "lecteur"
            && a == "rapport_utilisateur"
            && c.as_deref() == Some("2026-11-01..2026-11-30")));
    }

    #[tokio::test]
    async fn bornes_de_periode_dans_le_fuseau_configure() {
        let b = banc();
        charger_jeu(&b);
        let cookie = b.connecter().await;
        // 00:30 à Paris le 1er novembre : dans novembre, pas dans octobre, alors qu'il est encore le 31 en UTC.
        let dans = b.get("/?du=2026-11-01&au=2026-11-01", Some(&cookie)).await;
        assert!(dans.texte.contains("<b>161</b>"));
        let avant = b.get("/?du=2026-10-31&au=2026-10-31", Some(&cookie)).await;
        assert!(avant.texte.contains("<b>0</b>"));
        // La date affichée suit le fuseau.
        let d = b.get("/utilisateurs/a.exemple", Some(&cookie)).await;
        assert!(d.texte.contains("01/11/2026 00:30"), "{}", d.texte);
    }

    #[tokio::test]
    async fn periode_invalide_refusee_sans_trace_de_lecture() {
        let b = banc();
        let cookie = b.connecter().await;
        for q in ["du=hier", "du=2026-13-01", "du=2026-11-10&au=2026-11-01"] {
            for chemin in ["/", "/palmares", "/rapports/service", "/rapports/service/csv"] {
                let r = b.get(&format!("{chemin}?{q}"), Some(&cookie)).await;
                assert_eq!(r.code, StatusCode::BAD_REQUEST, "{chemin}?{q}");
            }
        }
        assert_eq!(b.journal().len(), 1, "seule la connexion");
    }

    #[tokio::test]
    async fn export_csv_journalise_et_sans_nom_de_document() {
        let b = banc();
        charger_jeu(&b);
        let cookie = b.connecter().await;
        let r = b
            .get(&format!("/rapports/utilisateur/csv?{NOVEMBRE}"), Some(&cookie))
            .await;
        assert_eq!(r.code, StatusCode::OK);
        assert!(
            r.entetes
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/csv")
        );
        assert_eq!(
            r.entetes.get(header::CONTENT_DISPOSITION).unwrap(),
            "attachment; filename=\"tirage-utilisateur-2026-11-01-2026-11-30.csv\""
        );
        assert!(
            r.texte.contains("a.exemple;A <b>Exemple</b>;2;110;100;10;0;"),
            "{}",
            r.texte
        );
        assert!(!r.texte.contains("confidentiel"));
        assert_eq!(b.journal()[0].1, "export_utilisateur");
    }

    #[tokio::test]
    async fn palmares_classe_et_echappe() {
        let b = banc();
        charger_jeu(&b);
        let cookie = b.connecter().await;
        let r = b.get(&format!("/palmares?{NOVEMBRE}"), Some(&cookie)).await;
        assert_eq!(r.code, StatusCode::OK);
        assert!(!r.texte.contains("<b>Exemple"));
        assert!(!r.texte.contains("confidentiel"));
        // Couleur : B (50 pages) devant A (10 pages).
        let couleur = r.texte.split("pages couleur").nth(1).unwrap();
        let avant_cout = couleur.split("coût").next().unwrap();
        assert!(avant_cout.find("B Exemple").unwrap() < avant_cout.find("A &lt;b&gt;").unwrap());
        assert_eq!(b.journal()[0].1, "palmares");
    }

    #[tokio::test]
    async fn etat_des_copieurs() {
        let b = banc();
        let cookie = b.connecter().await;
        let r = b.get("/copieurs", Some(&cookie)).await;
        assert!(r.texte.contains("Aucun relevé"));
        b.base
            .lock()
            .unwrap()
            .enregistrer(
                &Evenement::Releve(tirage_modele::ReleveCopieur {
                    collecteur: "impression-1".into(),
                    copieur: "COPIEUR-<1>".into(),
                    releve_ms: MAINTENANT,
                    modele: Some("Modèle fictif".into()),
                    numero_serie: None,
                    compteur_total: Some(12_345),
                    compteur_noir: None,
                    compteur_couleur: None,
                    consommables: vec![tirage_modele::Consommable {
                        description: "Noir".into(),
                        niveau: Some(40),
                        capacite: Some(100),
                    }],
                    etat: Some("prêt".into()),
                    alertes: vec![],
                }),
                0,
            )
            .unwrap();
        let r = b.get("/copieurs", Some(&cookie)).await;
        assert!(r.texte.contains("COPIEUR-&lt;1&gt;"));
        assert!(r.texte.contains("12\u{202f}345"));
        assert!(r.texte.contains("Noir : 40 %"));
    }

    #[test]
    fn nombres_lisibles() {
        assert_eq!(entier(0), "0");
        assert_eq!(entier(999), "999");
        assert_eq!(entier(1234567), "1\u{202f}234\u{202f}567");
        assert_eq!(pourcentage(1, 3), "33 %");
        assert_eq!(pourcentage(0, 0), "—");
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
