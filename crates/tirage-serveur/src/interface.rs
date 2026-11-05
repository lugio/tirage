//! Interface web : connexion réservée au groupe autorisé, tableaux, palmarès, coûts, export CSV, détail par
//! utilisateur, état des copieurs, journal des consultations.
//!
//! - Toute page de données passe par `exiger_session`, posé sur le routeur : une route ajoutée à `protegees` ne peut
//!   pas l'oublier.
//! - La consultation est écrite au journal **avant** la lecture des données : si le journal est indisponible, la
//!   page n'est pas servie.
//! - Tout texte venu des travaux ou de l'annuaire est échappé (un titre de document peut contenir du HTML).
//! - Cookie `SameSite=Strict` : un autre site ne peut pas déclencher d'action d'une session ouverte.

mod gabarit;
mod graphique;

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
use crate::base::{ActiviteCopieur, Base, Filtre};
use crate::config::{ConfigCouts, ConfigInterface};
use crate::conservation::limite_ms;
use crate::rapports::{self, Axe, Ligne, Tranche};
use crate::session::{Limiteur, Session, Sessions};
pub use gabarit::echapper;
use gabarit::{
    Cellule, Colonne, FEUILLE_DE_STYLE, Rubrique, Tableau, bouton_lien, carte, duree, encoder_chemin, entier,
    erreur, note, onglets, page, taille, tuiles,
};
use graphique::{Point, Serie, barre, chaleur, colonnes_empilees, jauge, legende, segments};

const COOKIE: &str = "tirage_session";
const CORPS_MAX: usize = 16 * 1024;
const LONGUEUR_MAX: usize = 1024;
const LIGNES_DETAIL: u32 = 500;
const LIGNES_JOURNAL: u32 = 500;
/// Lecteur consigné pour un essai refusé dont l'identifiant n'est pas connu : ce qui a été saisi n'est pas gardé
/// (on saisit parfois son mot de passe dans le mauvais champ).
const LECTEUR_INCONNU: &str = "(inconnu)";
const PERIODE_JOURS_MAX: i32 = 3660;
const SERIE_NOIR: Serie = Serie {
    nom: "N&B",
    classe: "noir",
};
const SERIE_COULEUR: Serie = Serie {
    nom: "Couleur",
    classe: "couleur",
};
const SERIE_INCONNU: Serie = Serie {
    nom: "Mode inconnu",
    classe: "inconnu",
};
const SERIES: [Serie; 3] = [SERIE_NOIR, SERIE_COULEUR, SERIE_INCONNU];
/// Lignes de chaque classement du tableau de bord.
const APERCU_TAILLE: usize = 5;

const PALMARES_TAILLE: usize = 10;
/// En dessous, l'activité horaire et les gros travaux d'un copieur décriraient une personne : ils ne s'affichent pas.
const PERSONNES_MIN_COPIEUR: i64 = 3;
/// Une réception plus ancienne ne compte plus : un collecteur retiré finit par sortir de l'état de la collecte.
const COLLECTE_JOURS: i64 = 30;
const JOURS_SEMAINE: [&str; 7] = ["Lun", "Mar", "Mer", "Jeu", "Ven", "Sam", "Dim"];

pub struct Interface {
    annuaire: Arc<dyn Annuaire>,
    sessions: Sessions,
    limiteur: Limiteur,
    cookie_securise: bool,
    fuseau: TimeZone,
    couts: ConfigCouts,
    detail_mois: u32,
    gros_travail_pages: u32,
    collecte_silence_minutes: u32,
    heures_ouvrees: (u8, u8),
}

impl Interface {
    pub fn nouvelle(
        annuaire: Arc<dyn Annuaire>,
        c: &ConfigInterface,
        couts: ConfigCouts,
        detail_mois: u32,
    ) -> Self {
        Interface {
            annuaire,
            sessions: Sessions::nouvelles(c.session_minutes, c.inactivite_minutes),
            limiteur: Limiteur::default(),
            cookie_securise: !c.http_en_clair,
            // La configuration a déjà validé le nom du fuseau.
            fuseau: TimeZone::get(&c.fuseau).unwrap_or(TimeZone::UTC),
            couts,
            detail_mois,
            gros_travail_pages: c.gros_travail_pages,
            collecte_silence_minutes: c.collecte_silence_minutes,
            heures_ouvrees: (c.heure_ouverture, c.heure_fermeture),
        }
    }

    /// État de la collecte : ne contient aucune donnée personnelle.
    fn etat_collecte(&self, receptions: &[(String, i64)], maintenant_ms: i64) -> String {
        if receptions.is_empty() {
            return erreur(&format!(
                "Collecte : aucun travail reçu depuis {COLLECTE_JOURS} jours. Vérifier le collecteur sur le serveur d'impression."
            ));
        }
        let seuil = i64::from(self.collecte_silence_minutes);
        let mut h = String::new();
        for (collecteur, recu_ms) in receptions {
            let silence =
                rapports::minutes_ouvrees(*recu_ms, maintenant_ms, &self.fuseau, self.heures_ouvrees, seuil);
            let quand = self.heure(*recu_ms);
            h.push_str(&if silence >= seuil {
                erreur(&format!(
                    "Collecte muette : {collecteur} n'a envoyé aucun travail depuis le {quand}, soit au moins {} ouvrées. \
                     Vérifier le collecteur ; un jour férié ou une période sans impression donne le même signe.",
                    duree(seuil * 60_000)
                ))
            } else {
                note(&format!("Collecte : {collecteur}, dernier travail reçu le {quand}."))
            });
        }
        h
    }

    /// Avis si la période remonte avant la limite de conservation du détail.
    fn avis_conservation(&self, p: &Periode, maintenant_ms: i64) -> String {
        match limite_ms(maintenant_ms, self.detail_mois, &self.fuseau) {
            Some(limite) if p.du_ms < limite => note(&format!(
                "Le détail est effacé au-delà de {} mois : avant cette limite, seuls comptent des totaux sans nom, \
                 et seulement pour les mois entiers de la période.",
                self.detail_mois
            )),
            _ => String::new(),
        }
    }

    /// Formulaire de période et raccourcis.
    fn selecteur(&self, p: &Periode, action: &str, maintenant_ms: i64) -> String {
        let mut h = p.formulaire(action);
        let Ok(t) = Timestamp::from_millisecond(maintenant_ms) else {
            return h;
        };
        let jour = t.to_zoned(self.fuseau.clone()).date();
        let mois = jour.first_of_month();
        let fin_precedent = mois.yesterday().unwrap_or(mois);
        let plages = [
            ("Aujourd'hui", Some(jour), jour),
            ("7 jours", jour.checked_sub(6.days()).ok(), jour),
            ("Ce mois", Some(mois), jour),
            (
                "Mois dernier",
                Some(fin_precedent.first_of_month()),
                fin_precedent,
            ),
            (
                "12 mois",
                jour.checked_sub(12.months()).and_then(|d| d.tomorrow()).ok(),
                jour,
            ),
        ];
        let liens: Vec<(String, &str, bool)> = plages
            .iter()
            .filter_map(|(libelle, du, au)| {
                let du = (*du)?;
                let actif = p.du == du && p.au == *au;
                Some((format!("{action}?du={du}&au={au}"), *libelle, actif))
            })
            .collect();
        h.push_str(&onglets(&liens));
        h
    }

    /// La période de même durée juste avant `p`, si le détail y est encore complet.
    fn periode_precedente(&self, p: &Periode, maintenant_ms: i64) -> Option<Periode> {
        let jours = p.du.until(p.au).ok()?.get_days();
        let au = p.du.yesterday().ok()?;
        let du = au.checked_sub(i64::from(jours).days()).ok()?;
        let demande = Demande {
            du: Some(du.to_string()),
            au: Some(au.to_string()),
        };
        let precedente = Periode::lire(&demande, maintenant_ms, &self.fuseau).ok()?;
        let limite = limite_ms(maintenant_ms, self.detail_mois, &self.fuseau)?;
        (precedente.du_ms >= limite).then_some(precedente)
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
        .route("/copieurs/{copieur}", get(vue_copieur))
        .route("/services/{service}", get(vue_service))
        .route("/cabinets/{cabinet}", get(vue_cabinet))
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
            Rubrique::Aucune,
            "Indisponible",
            "<p>Service indisponible, réessayer plus tard.</p>",
            None,
        )),
    )
        .into_response()
}

fn formulaire(message: Option<&str>) -> String {
    let m = message.map_or(String::new(), erreur);
    page(
        Rubrique::Aucune,
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
        // Le calcul de l'évolution parcourt la période, base verrouillée : une date extrême ne doit pas la bloquer.
        if du.until(au).map_or(true, |s| s.get_days() > PERIODE_JOURS_MAX) {
            return Err("La période ne peut pas dépasser dix ans.".into());
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

    /// Premier et dernier mois entièrement compris dans la période (« AAAA-MM ») : les totaux archivés sont mensuels.
    fn mois_entiers(&self) -> Option<(String, String)> {
        let premier = if self.du.day() == 1 {
            self.du
        } else {
            self.du.last_of_month().tomorrow().ok()?
        };
        let dernier = if self.au == self.au.last_of_month() {
            self.au
        } else {
            self.au.first_of_month().yesterday().ok()?
        };
        (premier <= dernier).then(|| {
            (
                premier.strftime("%Y-%m").to_string(),
                dernier.strftime("%Y-%m").to_string(),
            )
        })
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
        Html(page(Rubrique::Aucune, "Demande invalide", &erreur(message), s)),
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
    let mois = periode.mois_entiers();
    let interface = Arc::clone(&c.interface);
    avec_base(&c.base, move |b| {
        // Journal d'abord : sans trace, pas de données.
        b.consigner(maintenant, &lecteur, &action, Some(&cible))?;
        let mut lignes = Vec::with_capacity(axes.len());
        for axe in axes {
            let mut brutes = b.agreger(axe, du, au)?;
            if let Some((premier, dernier)) = &mois {
                brutes.extend(b.totaux_archives(axe, premier, dernier)?);
            }
            lignes.push(rapports::construire(&brutes, &interface.couts));
        }
        Ok((lignes, b.non_imprimes(du, au)?))
    })
    .await
}

fn pourcentage(partie: i64, tout: i64) -> String {
    if tout == 0 {
        return "—".into();
    }
    format!("{} %", (partie * 100 + tout / 2) / tout)
}

fn note_sur_le_calcul(total: &Ligne, non_imprimes: i64, devise: &str) -> String {
    let mut n = String::from("Seuls les travaux imprimés sont comptés.");
    if non_imprimes > 0 {
        n += &format!(
            " {} travaux annulés ou en erreur sont exclus.",
            entier(non_imprimes)
        );
    }
    if total.couleur_inconnue > 0 {
        n += &format!(
            " {} pages au mode couleur inconnu sont chiffrées au tarif noir.",
            entier(total.couleur_inconnue)
        );
    }
    if total.sans_tarif > 0 {
        n += &format!(
            " {} pages sans tarif ne sont pas dans le coût : il est donc incomplet ({devise}).",
            entier(total.sans_tarif)
        );
    }
    note(&n)
}

/// Accès aux rapports par axe, sur la période affichée.
fn onglets_rapports(periode: &Periode, actif: Option<Axe>) -> String {
    let liens: Vec<(String, &str, bool)> = Axe::TOUS
        .iter()
        .map(|a| {
            (
                format!("/rapports/{}?{}", a.chemin(), periode.requete()),
                a.titre(),
                actif == Some(*a),
            )
        })
        .collect();
    onglets(&liens)
}

/// Les chiffres d'une ligne de rapport, dans l'ordre des colonnes.
fn cellules_chiffres(l: &Ligne) -> impl Iterator<Item = Cellule> {
    [
        l.travaux,
        l.pages,
        l.noir,
        l.couleur,
        l.couleur_inconnue,
        l.recto_verso,
        l.feuilles,
    ]
    .into_iter()
    .map(|v| Cellule::Nombre(entier(v)))
    .chain([Cellule::Nombre(rapports::montant(l.cout))])
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
    let axes = vec![Axe::Service, Axe::Copieur];
    let (lignes, non_imprimes) = match lire_rapports(&c, &s.login, "tableau".into(), &periode, axes).await {
        Ok(v) => v,
        Err(rep) => return *rep,
    };
    let (tranches, temps) = match lire_evolution(&c, &periode).await {
        Ok(v) => v,
        Err(rep) => return *rep,
    };
    let receptions = match lire_receptions(&c).await {
        Ok(v) => v,
        Err(rep) => return *rep,
    };
    let vide = Vec::new();
    let (services, copieurs) = (lignes.first().unwrap_or(&vide), lignes.get(1).unwrap_or(&vide));
    let t = rapports::total(services);
    let maintenant = (c.horloge)();
    let mut h = i.etat_collecte(&receptions, maintenant);
    h.push_str(&i.selecteur(&periode, "/", maintenant));
    h.push_str(&tuiles_de(&t, &i.couts.devise));
    if let Some(precedente) = i.periode_precedente(&periode, maintenant) {
        match lire_total(&c, &precedente).await {
            Ok(avant) => h.push_str(&comparaison(&t, &avant, &precedente)),
            Err(rep) => return *rep,
        }
    }
    h.push_str(&note_sur_le_calcul(&t, non_imprimes, &i.couts.devise));
    h.push_str(&i.avis_conservation(&periode, (c.horloge)()));
    h.push_str(&carte(
        "Pages imprimées dans le temps",
        &bloc_evolution(&tranches),
    ));
    h.push_str(&carte(
        "Activité par jour et par heure",
        &bloc_activite(&temps, &i.fuseau),
    ));
    h.push_str("<div class=\"grille\">");
    h.push_str(&carte("Couleur et recto-verso", &bloc_modes(&t)));
    h.push_str(&liste_palmares(
        "Services : pages",
        services,
        Ligne::lire_pages,
        entier,
        APERCU_TAILLE,
    ));
    h.push_str(&liste_palmares(
        "Copieurs : pages",
        copieurs,
        Ligne::lire_pages,
        entier,
        APERCU_TAILLE,
    ));
    h.push_str("</div><h2>Rapports détaillés</h2>");
    h.push_str(&onglets_rapports(&periode, None));
    h.push_str(&note("Chaque consultation est inscrite au journal."));
    Html(page(Rubrique::Accueil, "Tableau de bord", &h, Some(&s))).into_response()
}

/// Total d'une période, détail et mois archivés. Sans nom : la consultation est déjà inscrite par l'appelant.
async fn lire_total(c: &Contexte, p: &Periode) -> Result<Ligne, Box<Response>> {
    let (du, au, mois) = (p.du_ms, p.au_ms, p.mois_entiers());
    let interface = Arc::clone(&c.interface);
    avec_base(&c.base, move |b| {
        let mut brutes = b.agreger(Axe::Service, du, au)?;
        if let Some((premier, dernier)) = &mois {
            brutes.extend(b.totaux_archives(Axe::Service, premier, dernier)?);
        }
        Ok(rapports::total(&rapports::construire(&brutes, &interface.couts)))
    })
    .await
}

/// Les chiffres en tête de page.
fn tuiles_de(t: &Ligne, devise: &str) -> String {
    tuiles(&[
        (entier(t.pages), "pages imprimées"),
        (entier(t.feuilles), "feuilles (estimées au besoin)"),
        (pourcentage(t.couleur, t.pages), "des pages en couleur"),
        (pourcentage(t.recto_verso, t.pages), "des pages en recto-verso"),
        (
            entier((t.pages - t.feuilles).max(0)),
            "feuilles évitées (recto-verso)",
        ),
        (format!("{} {devise}", rapports::montant(t.cout)), "coût"),
    ])
}

/// Écart en pourcentage de `avant`, arrondi et signé. `None` si `avant` est nul.
fn ecart(actuel: i64, avant: i64) -> Option<String> {
    if avant <= 0 {
        return None;
    }
    let brut = (actuel - avant) * 100;
    let v = (brut + brut.signum() * avant / 2) / avant;
    Some(match v {
        0 => "stable".to_owned(),
        v if v > 0 => format!("+{v} %"),
        v => format!("\u{2212}{} %", -v),
    })
}

/// Comparaison à la période précédente ; rien s'il n'y a rien à comparer.
fn comparaison(t: &Ligne, avant: &Ligne, precedente: &Periode) -> String {
    let (Some(pages), Some(feuilles)) = (ecart(t.pages, avant.pages), ecart(t.feuilles, avant.feuilles))
    else {
        return String::new();
    };
    note(&format!(
        "Par rapport à la période précédente (du {} au {}) : pages {pages}, feuilles {feuilles}.",
        precedente.du.strftime("%d/%m/%Y"),
        precedente.au.strftime("%d/%m/%Y")
    ))
}

/// Part de la couleur, puis part du recto-verso : deux mesures distinctes, d'où deux couleurs distinctes.
fn bloc_modes(t: &Ligne) -> String {
    if t.pages <= 0 {
        return note("Aucune page sur cette période.");
    }
    let texte = format!(
        "Recto-verso : {} pages sur {} ({})",
        entier(t.recto_verso),
        entier(t.pages),
        pourcentage(t.recto_verso, t.pages)
    );
    format!(
        "{}<p class=\"sous-mesure\">{}</p>{}",
        bloc_repartition(t),
        echapper(&texte),
        jauge(t.recto_verso, t.pages, &texte)
    )
}

/// Carte de chaleur sur le détail encore conservé. Jamais pour une personne : seuls le tableau de bord et la vue d'un
/// copieur, à partir de quelques personnes, l'appellent.
fn bloc_activite(temps: &[(i64, String, i64)], fuseau: &TimeZone) -> String {
    let grille = rapports::activite_horaire(temps, fuseau);
    // Les heures utiles, journée de bureau au moins ; le week-end seulement s'il y a eu quelque chose.
    let utile = |h: usize| grille.iter().any(|j| j[h] > 0);
    let debut = (0..24).find(|h| utile(*h)).unwrap_or(8).min(8);
    let fin = (0..24).rfind(|h| utile(*h)).unwrap_or(18).max(18);
    let colonnes: Vec<String> = (debut..=fin).map(|h| format!("{h} h")).collect();
    let lignes: Vec<(&str, Vec<i64>)> = JOURS_SEMAINE
        .iter()
        .zip(&grille)
        .enumerate()
        .filter(|(n, (_, j))| *n < 5 || j.iter().any(|v| *v > 0))
        .map(|(_, (nom, j))| (*nom, j[debut..=fin].to_vec()))
        .collect();
    let Some(svg) = chaleur(
        "Pages imprimées par jour de la semaine et par heure",
        "pages",
        &colonnes,
        &lignes,
    ) else {
        return note("Aucune page sur cette période.");
    };
    let mut entete = vec![Colonne::texte("Jour")];
    entete.extend(colonnes.iter().map(|c| Colonne::nombre(c)));
    let mut tableau = Tableau::nouveau(&entete);
    for (nom, valeurs) in &lignes {
        tableau.ligne(
            std::iter::once(Cellule::texte(nom)).chain(valeurs.iter().map(|v| Cellule::Nombre(entier(*v)))),
        );
    }
    format!(
        "{svg}{}<details><summary>Voir les chiffres</summary>{}</details>",
        note("Pages cumulées sur la période : plus la case est foncée, plus il y en a."),
        tableau.fin()
    )
}

/// Dernières réceptions, sans donnée personnelle : rien à inscrire au journal.
async fn lire_receptions(c: &Contexte) -> Result<Vec<(String, i64)>, Box<Response>> {
    let depuis = (c.horloge)() - COLLECTE_JOURS * 86_400_000;
    avec_base(&c.base, move |b| b.dernieres_receptions(depuis)).await
}

/// Pages dans le temps, en tranches et brutes. Lu après `lire_rapports`, qui a déjà inscrit la consultation au journal.
async fn lire_evolution(
    c: &Contexte,
    p: &Periode,
) -> Result<(Vec<Tranche>, Vec<(i64, String, i64)>), Box<Response>> {
    let (du, au, du_ms, au_ms, mois) = (p.du, p.au, p.du_ms, p.au_ms, p.mois_entiers());
    let interface = Arc::clone(&c.interface);
    avec_base(&c.base, move |b| {
        let detail = b.pages_dans_le_temps(du_ms, au_ms)?;
        let archives = match &mois {
            Some((premier, dernier)) => b.pages_archivees(premier, dernier)?,
            None => Vec::new(),
        };
        let tranches = rapports::evolution(du, au, &interface.fuseau, &detail, &archives);
        Ok((tranches, detail))
    })
    .await
}

/// Le graphique et ses chiffres, repliés dans un tableau pour qui ne peut pas lire le graphique.
fn bloc_evolution(tranches: &[Tranche]) -> String {
    let points: Vec<Point> = tranches
        .iter()
        .map(|t| Point {
            libelle: t.libelle.clone(),
            valeurs: vec![t.noir, t.couleur, t.inconnu],
        })
        .collect();
    let Some(svg) = colonnes_empilees("Pages imprimées par tranche de temps", "pages", &SERIES, &points)
    else {
        return note("Aucune page sur cette période.");
    };
    let mut tableau = Tableau::nouveau(&[
        Colonne::texte("Période"),
        Colonne::nombre(SERIE_NOIR.nom),
        Colonne::nombre(SERIE_COULEUR.nom),
        Colonne::nombre(SERIE_INCONNU.nom),
    ]);
    for t in tranches {
        tableau.ligne([
            Cellule::texte(&t.libelle),
            Cellule::Nombre(entier(t.noir)),
            Cellule::Nombre(entier(t.couleur)),
            Cellule::Nombre(entier(t.inconnu)),
        ]);
    }
    let inconnu = tranches.iter().any(|t| t.inconnu > 0);
    format!(
        "{}{svg}<details><summary>Voir les chiffres</summary>{}</details>",
        legende(series_presentes(inconnu)),
        tableau.fin()
    )
}

/// Le mode inconnu reste exceptionnel : il n'entre dans la légende que s'il y en a.
fn series_presentes(inconnu: bool) -> &'static [Serie] {
    if inconnu { &SERIES } else { &SERIES[..2] }
}

fn bloc_repartition(t: &Ligne) -> String {
    let parts = [
        (SERIE_NOIR, t.noir),
        (SERIE_COULEUR, t.couleur),
        (SERIE_INCONNU, t.couleur_inconnue),
    ];
    match segments("Part des pages en noir et blanc et en couleur", &parts) {
        Some(svg) => format!("{svg}{}", legende(series_presentes(t.couleur_inconnue > 0))),
        None => note("Aucune page sur cette période."),
    }
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
    let mut h = i.selecteur(&periode, &format!("/rapports/{}", axe.chemin()), (c.horloge)());
    h.push_str(&onglets_rapports(&periode, Some(axe)));
    h.push_str(&note_sur_le_calcul(&t, non_imprimes, &i.couts.devise));
    if axe == Axe::Application {
        h.push_str(&note(
            "L'application est déduite du titre du document : elle reste inconnue quand le titre ne la dit pas. \
             Les totaux archivés ne la gardent pas.",
        ));
    }
    h.push_str(&i.avis_conservation(&periode, (c.horloge)()));
    h.push_str(&liste_palmares(
        "Les plus gros volumes",
        lignes,
        Ligne::lire_pages,
        entier,
        PALMARES_TAILLE,
    ));
    h.push_str(&tableau_axe(axe, lignes, &t, &i.couts.devise, &periode));
    h.push_str(&bouton_lien(
        &format!("/rapports/{}/csv?{}", axe.chemin(), periode.requete()),
        "Exporter en CSV",
    ));
    Html(page(
        Rubrique::Accueil,
        &format!("Par {}", axe.titre().to_lowercase()),
        &h,
        Some(&s),
    ))
    .into_response()
}

/// La clé d'une ligne, avec son lien quand l'axe a sa propre vue.
fn cellule_cle(axe: Axe, cle: &str, periode: &Periode) -> Cellule {
    let racine = match axe {
        Axe::Utilisateur => return Cellule::utilisateur(cle),
        Axe::Format | Axe::Application => return Cellule::texte(cle),
        Axe::Service => Vue::Service.chemin(),
        Axe::Cabinet => Vue::Cabinet.chemin(),
        Axe::Copieur => Vue::Copieur.chemin(),
    };
    Cellule::Lien {
        href: format!("{racine}/{}?{}", encoder_chemin(cle), periode.requete()),
        texte: cle.to_owned(),
    }
}

/// Le tableau d'un axe, avec sa ligne de total.
fn tableau_axe(axe: Axe, lignes: &[Ligne], t: &Ligne, devise: &str, periode: &Periode) -> String {
    let par_personne = axe == Axe::Utilisateur;
    let mut colonnes = vec![Colonne::texte(axe.titre())];
    if par_personne {
        colonnes.push(Colonne::texte("Nom"));
    }
    colonnes.extend(
        [
            "Travaux",
            "Pages",
            "N&B",
            "Couleur",
            "Mode inconnu",
            "Recto-verso",
            "Feuilles",
        ]
        .map(Colonne::nombre),
    );
    colonnes.push(Colonne::nombre(&format!("Coût ({devise})")));
    let mut tableau = Tableau::nouveau(&colonnes);
    for l in lignes {
        let mut tete = vec![cellule_cle(axe, &l.cle, periode)];
        if par_personne {
            tete.push(Cellule::texte(l.nom.as_deref().unwrap_or("")));
        }
        tableau.ligne(tete.into_iter().chain(cellules_chiffres(l)));
    }
    let tete = if par_personne {
        vec![Cellule::texte("Total"), Cellule::Vide]
    } else {
        vec![Cellule::texte("Total")]
    };
    tableau.total(tete.into_iter().chain(cellules_chiffres(t)));
    tableau.fin()
}

/// Vue d'un service, d'un cabinet ou d'un copieur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vue {
    Service,
    Cabinet,
    Copieur,
}

impl Vue {
    fn chemin(self) -> &'static str {
        match self {
            Vue::Service => "/services",
            Vue::Cabinet => "/cabinets",
            Vue::Copieur => "/copieurs",
        }
    }

    fn action(self) -> &'static str {
        match self {
            Vue::Service => "vue_service",
            Vue::Cabinet => "vue_cabinet",
            Vue::Copieur => "vue_copieur",
        }
    }

    fn titre(self) -> &'static str {
        match self {
            Vue::Service => "Service",
            Vue::Cabinet => "Cabinet",
            Vue::Copieur => "Copieur",
        }
    }

    fn filtre(self, valeur: &str) -> Filtre<'_> {
        match self {
            Vue::Service => Filtre::Service(valeur),
            Vue::Cabinet => Filtre::Cabinet(valeur),
            Vue::Copieur => Filtre::Copieur(valeur),
        }
    }

    /// Le premier axe donne le tableau, le second un classement.
    fn axes(self) -> (Axe, Axe) {
        match self {
            Vue::Service => (Axe::Utilisateur, Axe::Copieur),
            Vue::Cabinet => (Axe::Service, Axe::Copieur),
            Vue::Copieur => (Axe::Utilisateur, Axe::Service),
        }
    }
}

async fn vue_service(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    Path(valeur): Path<String>,
    Query(d): Query<Demande>,
) -> Response {
    vue(c, s, Vue::Service, valeur, d).await
}

async fn vue_cabinet(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    Path(valeur): Path<String>,
    Query(d): Query<Demande>,
) -> Response {
    vue(c, s, Vue::Cabinet, valeur, d).await
}

async fn vue_copieur(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    Path(valeur): Path<String>,
    Query(d): Query<Demande>,
) -> Response {
    vue(c, s, Vue::Copieur, valeur, d).await
}

/// Vrai si le copieur est assez partagé pour que son activité ne décrive pas une personne.
fn assez_de_personnes(a: &ActiviteCopieur) -> bool {
    a.personnes >= PERSONNES_MIN_COPIEUR
}

/// Activité horaire et gros travaux d'un copieur, sans nom, à partir de quelques personnes sur la période.
fn bloc_copieur_anonyme(
    i: &Interface,
    activite: Option<&ActiviteCopieur>,
    t: &Ligne,
    temps: &[(i64, String, i64)],
) -> String {
    let Some(a) = activite.filter(|a| assez_de_personnes(a)) else {
        return note(&format!(
            "Activité par heure et gros travaux : affichés à partir de {PERSONNES_MIN_COPIEUR} personnes sur la période, \
             pour ne décrire personne en particulier."
        ));
    };
    let gros = format!(
        "Travaux d'au moins {} pages : {}, soit {} pages et {} des pages du copieur.",
        entier(i64::from(i.gros_travail_pages)),
        entier(a.gros_travaux),
        entier(a.gros_pages),
        pourcentage(a.gros_pages, t.pages)
    );
    format!(
        "{}{}",
        carte("Activité par jour et par heure", &bloc_activite(temps, &i.fuseau)),
        carte("Gros travaux", &format!("<p>{}</p>", echapper(&gros)))
    )
}

/// Détail seul : les totaux archivés, sans nom, ne se découpent pas par personne.
async fn vue(c: Contexte, s: Session, vue: Vue, valeur: String, d: Demande) -> Response {
    let i = Arc::clone(&c.interface);
    let maintenant = (c.horloge)();
    let periode = match Periode::lire(&d, maintenant, &i.fuseau) {
        Ok(p) => p,
        Err(m) => return demande_invalide(&m, Some(&s)),
    };
    let action = format!("{}/{}", vue.chemin(), encoder_chemin(&valeur));
    let (lecteur, du, au, cible) = (
        s.login.clone(),
        periode.du_ms,
        periode.au_ms,
        format!("{valeur} {}", periode.texte()),
    );
    let (valeur_lue, interface) = (valeur.clone(), Arc::clone(&i));
    let lu = avec_base(&c.base, move |b| {
        // Journal d'abord : sans trace, pas de données.
        b.consigner(maintenant, &lecteur, vue.action(), Some(&cible))?;
        let filtre = vue.filtre(&valeur_lue);
        let (axe1, axe2) = vue.axes();
        let lignes1 = b.agreger_filtre(axe1, du, au, filtre)?;
        let lignes2 = b.agreger_filtre(axe2, du, au, filtre)?;
        let temps = b.pages_dans_le_temps_filtre(du, au, filtre)?;
        let activite = if vue == Vue::Copieur {
            b.activite_copieurs(du, au, i64::from(interface.gros_travail_pages))?
                .into_iter()
                .find(|a| a.file.eq_ignore_ascii_case(&valeur_lue))
        } else {
            None
        };
        Ok((
            rapports::construire(&lignes1, &interface.couts),
            rapports::construire(&lignes2, &interface.couts),
            temps,
            activite,
        ))
    })
    .await;
    let (lignes1, lignes2, temps, activite) = match lu {
        Ok(v) => v,
        Err(rep) => return *rep,
    };
    let t = rapports::total(&lignes1);
    let (axe1, axe2) = vue.axes();
    let tranches = rapports::evolution(periode.du, periode.au, &i.fuseau, &temps, &[]);
    let mut h = i.selecteur(&periode, &action, maintenant);
    h.push_str(&tuiles_de(&t, &i.couts.devise));
    h.push_str(&note(
        "Seuls les travaux imprimés encore en détail sont comptés. Consultation inscrite au journal.",
    ));
    h.push_str(&i.avis_conservation(&periode, maintenant));
    h.push_str(&carte(
        "Pages imprimées dans le temps",
        &bloc_evolution(&tranches),
    ));
    if vue == Vue::Copieur {
        h.push_str(&bloc_copieur_anonyme(&i, activite.as_ref(), &t, &temps));
    }
    h.push_str("<div class=\"grille\">");
    h.push_str(&carte("Couleur et recto-verso", &bloc_modes(&t)));
    h.push_str(&liste_palmares(
        &format!("{} : pages", axe2.titre()),
        &lignes2,
        Ligne::lire_pages,
        entier,
        PALMARES_TAILLE,
    ));
    h.push_str("</div>");
    h.push_str(&tableau_axe(axe1, &lignes1, &t, &i.couts.devise, &periode));
    let rubrique = if vue == Vue::Copieur {
        Rubrique::Copieurs
    } else {
        Rubrique::Accueil
    };
    Html(page(
        rubrique,
        &format!("{} : {valeur}", vue.titre()),
        &h,
        Some(&s),
    ))
    .into_response()
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

fn liste_palmares(
    titre: &str,
    tout: &[Ligne],
    mesure: fn(&Ligne) -> i64,
    valeur: fn(i64) -> String,
    taille: usize,
) -> String {
    let lignes = rapports::palmares(tout, mesure, taille);
    let max = lignes.first().map_or(0, |l| mesure(l));
    let mut tableau = Tableau::sans_entete();
    for (rang, l) in lignes.iter().enumerate() {
        let nom = l.nom.as_deref().unwrap_or(&l.cle);
        let v = valeur(mesure(l));
        tableau.ligne([
            Cellule::Nombre((rang + 1).to_string()),
            Cellule::texte(nom),
            Cellule::Html(barre(mesure(l), max, &format!("{nom} : {v}"))),
            Cellule::Nombre(v.clone()),
        ]);
    }
    carte(titre, &tableau.fin_ou("Rien sur cette période."))
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
    h.push_str(&c.interface.avis_conservation(&periode, (c.horloge)()));
    h.push_str("<div class=\"grille\">");
    type Mesure = fn(&Ligne) -> i64;
    type Valeur = fn(i64) -> String;
    let classements: [(&str, &Vec<Ligne>, Mesure, Valeur); 5] = [
        ("Utilisateurs : pages", u, Ligne::lire_pages, entier),
        ("Utilisateurs : pages couleur", u, Ligne::lire_couleur, entier),
        ("Utilisateurs : coût", u, Ligne::lire_cout, rapports::montant),
        ("Services : pages", sv, Ligne::lire_pages, entier),
        ("Copieurs : pages", co, Ligne::lire_pages, entier),
    ];
    for (titre, lignes, mesure, valeur) in classements {
        h.push_str(&liste_palmares(titre, lignes, mesure, valeur, PALMARES_TAILLE));
    }
    h.push_str("</div>");
    Html(page(Rubrique::Palmares, "Palmarès", &h, Some(&s))).into_response()
}

/// Activité par copieur, tirée des travaux : ne dépend pas du relevé SNMP.
fn tableau_activite(
    i: &Interface,
    lignes: &[Ligne],
    activite: &[ActiviteCopieur],
    periode: &Periode,
) -> String {
    let mut tableau = Tableau::nouveau(&[
        Colonne::texte("Copieur"),
        Colonne::nombre("Travaux"),
        Colonne::nombre("Pages"),
        Colonne::nombre("N&B"),
        Colonne::nombre("Couleur"),
        Colonne::nombre("Recto-verso"),
        Colonne::nombre("Feuilles"),
        Colonne::nombre("Personnes"),
        Colonne::nombre("Non imprimés"),
        Colonne::nombre("Gros travaux"),
        Colonne::texte("Dernier travail"),
    ]);
    let cherche = |file: &str| activite.iter().find(|a| a.file.eq_ignore_ascii_case(file));
    let vide = Ligne::default();
    let sans_page = activite
        .iter()
        .filter(|a| !lignes.iter().any(|l| l.cle.eq_ignore_ascii_case(&a.file)))
        .map(|a| (a.file.as_str(), &vide));
    let rangs = lignes.iter().map(|l| (l.cle.as_str(), l)).chain(sans_page);
    for (file, l) in rangs {
        let a = cherche(file);
        tableau.ligne([
            cellule_cle(Axe::Copieur, file, periode),
            Cellule::Nombre(entier(l.travaux)),
            Cellule::Nombre(entier(l.pages)),
            Cellule::Nombre(entier(l.noir)),
            Cellule::Nombre(entier(l.couleur)),
            Cellule::Nombre(pourcentage(l.recto_verso, l.pages)),
            Cellule::Nombre(entier(l.feuilles)),
            Cellule::Nombre(a.map_or("—".to_owned(), |a| entier(a.personnes))),
            Cellule::Nombre(a.map_or("—".to_owned(), |a| entier(a.non_imprimes))),
            Cellule::Nombre(
                a.filter(|a| assez_de_personnes(a))
                    .map_or("—".to_owned(), |a| entier(a.gros_travaux)),
            ),
            Cellule::Texte(a.map_or(String::new(), |a| i.heure(a.dernier_ms))),
        ]);
    }
    tableau.fin_ou("Aucun travail sur cette période.")
}

async fn copieurs(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    Query(d): Query<Demande>,
) -> Response {
    let maintenant = (c.horloge)();
    let i = Arc::clone(&c.interface);
    let periode = match Periode::lire(&d, maintenant, &i.fuseau) {
        Ok(p) => p,
        Err(m) => return demande_invalide(&m, Some(&s)),
    };
    let lu = lire_rapports(&c, &s.login, "copieurs".into(), &periode, vec![Axe::Copieur]).await;
    let (lignes, non_imprimes) = match lu {
        Ok(v) => v,
        Err(rep) => return *rep,
    };
    let lignes = lignes.first().map_or(&[][..], Vec::as_slice);
    let (du, au, seuil) = (periode.du_ms, periode.au_ms, i64::from(i.gros_travail_pages));
    let lu = avec_base(&c.base, move |b| {
        Ok((b.activite_copieurs(du, au, seuil)?, b.derniers_releves()?))
    })
    .await;
    let (activite, releves) = match lu {
        Ok(r) => r,
        Err(rep) => return *rep,
    };
    let receptions = match lire_receptions(&c).await {
        Ok(v) => v,
        Err(rep) => return *rep,
    };
    let t = rapports::total(lignes);
    let mut h = i.etat_collecte(&receptions, maintenant);
    h.push_str(&i.selecteur(&periode, "/copieurs", maintenant));
    h.push_str(&note_sur_le_calcul(&t, non_imprimes, &i.couts.devise));
    h.push_str(&i.avis_conservation(&periode, maintenant));
    h.push_str("<h2>Activité</h2>");
    h.push_str(&liste_palmares(
        "Pages par copieur",
        lignes,
        Ligne::lire_pages,
        entier,
        PALMARES_TAILLE,
    ));
    h.push_str(&tableau_activite(&i, lignes, &activite, &periode));
    h.push_str(&note(&format!(
        "Gros travaux : au moins {} pages, copies comprises. Affichés à partir de {PERSONNES_MIN_COPIEUR} personnes \
         sur la période, pour ne décrire personne en particulier.",
        entier(seuil)
    )));
    h.push_str("<h2>État relevé</h2>");
    let compteur = |v: Option<u64>| {
        Cellule::Nombre(v.map_or("—".to_owned(), |n| entier(i64::try_from(n).unwrap_or(i64::MAX))))
    };
    let mut tableau = Tableau::nouveau(&[
        Colonne::texte("Copieur"),
        Colonne::texte("Modèle"),
        Colonne::texte("Relevé le"),
        Colonne::nombre("Compteur total"),
        Colonne::nombre("N&B"),
        Colonne::nombre("Couleur"),
        Colonne::texte("État"),
        Colonne::texte("Consommables"),
        Colonne::texte("Alertes"),
    ]);
    for r in &releves {
        let conso: Vec<String> = r
            .consommables
            .iter()
            .map(|x| match x.pourcentage() {
                Some(p) => format!("{} : {p} %", x.description),
                None => x.description.clone(),
            })
            .collect();
        tableau.ligne([
            Cellule::texte(&r.copieur),
            Cellule::texte(r.modele.as_deref().unwrap_or("")),
            Cellule::Texte(c.interface.heure(r.releve_ms)),
            compteur(r.compteur_total),
            compteur(r.compteur_noir),
            compteur(r.compteur_couleur),
            Cellule::texte(r.etat.as_deref().unwrap_or("")),
            Cellule::Texte(conso.join(", ")),
            Cellule::Texte(r.alertes.join(", ")),
        ]);
    }
    h.push_str(&tableau.fin_ou("Aucun relevé reçu : le relevé SNMP des copieurs n'est pas encore actif."));
    Html(page(Rubrique::Copieurs, "Copieurs", &h, Some(&s))).into_response()
}

/// Libellés lisibles des valeurs du spouleur ; une valeur inconnue reste telle quelle.
fn libelle(v: &str) -> &str {
    match v {
        "noir_et_blanc" => "N&B",
        "couleur" => "Couleur",
        "recto" => "Recto",
        "bord_long" => "Bord long",
        "bord_court" => "Bord court",
        "imprime" => "Imprimé",
        "annule" => "Annulé",
        "erreur" => "Erreur",
        "inconnu" => "Inconnu",
        "portrait" => "Portrait",
        "paysage" => "Paysage",
        autre => autre,
    }
}

async fn detail(
    State(c): State<Contexte>,
    Extension(s): Extension<Session>,
    Path(utilisateur): Path<String>,
    Query(d): Query<Demande>,
) -> Response {
    let maintenant = (c.horloge)();
    let i = Arc::clone(&c.interface);
    // Sans date demandée : tout le détail encore conservé.
    let du_conserve = limite_ms(maintenant, i.detail_mois, &i.fuseau)
        .and_then(|ms| Timestamp::from_millisecond(ms).ok())
        .and_then(|t| t.to_zoned(i.fuseau.clone()).date().tomorrow().ok())
        .map(|d| d.to_string());
    let demande = Demande {
        du: d.du.filter(|v| !v.trim().is_empty()).or(du_conserve),
        au: d.au,
    };
    let periode = match Periode::lire(&demande, maintenant, &i.fuseau) {
        Ok(p) => p,
        Err(m) => return demande_invalide(&m, Some(&s)),
    };
    let lecteur = s.login.clone();
    let cible = utilisateur.clone();
    let (du, au, interface) = (periode.du_ms, periode.au_ms, Arc::clone(&i));
    let lu = avec_base(&c.base, move |b| {
        // Journal d'abord : sans trace, pas de données.
        b.consigner(maintenant, &lecteur, "detail_utilisateur", Some(&cible))?;
        let filtre = Filtre::Utilisateur(&cible);
        let par_copieur = b.agreger_filtre(Axe::Copieur, du, au, filtre)?;
        Ok((
            b.personne(&cible)?,
            b.travaux_de(&cible, du, au, LIGNES_DETAIL)?,
            rapports::construire(&par_copieur, &interface.couts),
            b.pages_dans_le_temps_filtre(du, au, filtre)?,
        ))
    })
    .await;
    let (personne, travaux, par_copieur, temps) = match lu {
        Ok(v) => v,
        Err(rep) => return *rep,
    };
    let mut h = personne.as_ref().map_or(String::new(), |p| {
        format!(
            "<p>{} — service : {} — cabinet : {}{}</p>",
            echapper(p.nom.as_deref().unwrap_or("nom inconnu")),
            echapper(p.service.as_deref().unwrap_or("inconnu")),
            echapper(p.cabinet.as_deref().unwrap_or("inconnu")),
            if p.trouve { "" } else { " — absent de l'annuaire" },
        )
    });
    let t = rapports::total(&par_copieur);
    let tranches = rapports::evolution(periode.du, periode.au, &i.fuseau, &temps, &[]);
    h.push_str(&i.selecteur(
        &periode,
        &format!("/utilisateurs/{}", encoder_chemin(&utilisateur)),
        maintenant,
    ));
    h.push_str(&tuiles_de(&t, &i.couts.devise));
    h.push_str(&note(
        "Seuls les travaux imprimés sont comptés dans les chiffres ; la liste les montre tous. Consultation inscrite au journal.",
    ));
    h.push_str(&carte(
        "Pages imprimées dans le temps",
        &bloc_evolution(&tranches),
    ));
    h.push_str("<div class=\"grille\">");
    h.push_str(&carte("Couleur et recto-verso", &bloc_modes(&t)));
    h.push_str(&liste_palmares(
        "Copieurs utilisés : pages",
        &par_copieur,
        Ligne::lire_pages,
        entier,
        PALMARES_TAILLE,
    ));
    h.push_str("</div><h2>Travaux</h2>");
    let mut tableau = Tableau::nouveau(&[
        Colonne::texte("Envoyé"),
        Colonne::texte("File"),
        Colonne::texte("Poste"),
        Colonne::nombre("Pages"),
        Colonne::nombre("Copies"),
        Colonne::nombre("Feuilles"),
        Colonne::texte("Couleur"),
        Colonne::texte("Recto-verso"),
        Colonne::texte("Format"),
        Colonne::texte("Statut"),
        Colonne::texte("Document"),
        Colonne::texte("Application"),
        Colonne::nombre("Durée"),
        Colonne::nombre("Taille"),
        Colonne::nombre("Résolution"),
        Colonne::texte("Orientation"),
    ]);
    for t in &travaux {
        tableau.ligne([
            Cellule::Texte(c.interface.heure(t.envoye_ms)),
            Cellule::texte(&t.file),
            Cellule::texte(t.poste.as_deref().unwrap_or("")),
            Cellule::Nombre(t.pages.to_string()),
            Cellule::Nombre(t.copies.to_string()),
            Cellule::Nombre(t.feuilles.to_string()),
            Cellule::texte(libelle(&t.couleur)),
            Cellule::texte(libelle(&t.recto_verso)),
            Cellule::texte(t.format.as_deref().unwrap_or("")),
            Cellule::texte(libelle(&t.statut)),
            Cellule::texte(t.nom_document.as_deref().unwrap_or("")),
            Cellule::texte(t.application.as_deref().unwrap_or("")),
            Cellule::Nombre(t.imprime_ms.map_or(String::new(), |f| duree(f - t.envoye_ms))),
            Cellule::Nombre(t.taille_octets.map_or(String::new(), taille)),
            Cellule::Nombre(t.resolution_ppp.map_or(String::new(), |r| format!("{r} ppp"))),
            Cellule::texte(libelle(t.orientation.as_deref().unwrap_or(""))),
        ]);
    }
    h.push_str(&tableau.fin_ou("Aucun travail pour cette personne sur cette période."));
    Html(page(
        Rubrique::Aucune,
        &format!("Détail : {utilisateur}"),
        &h,
        Some(&s),
    ))
    .into_response()
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
    let mut tableau = Tableau::nouveau(&[
        Colonne::texte("Quand"),
        Colonne::texte("Qui"),
        Colonne::texte("Action"),
        Colonne::texte("Sur"),
    ]);
    for l in &lignes {
        let sur = match &l.cible {
            // Lien seulement vers un détail : les autres lignes portent une période.
            Some(u) if l.action == "detail_utilisateur" => Cellule::utilisateur(u),
            Some(autre) => Cellule::texte(autre),
            None => Cellule::Vide,
        };
        tableau.ligne([
            Cellule::Texte(c.interface.heure(l.quand_ms)),
            Cellule::texte(&l.lecteur),
            Cellule::texte(&l.action),
            sur,
        ]);
    }
    let h = tableau.fin_ou("Aucune consultation.");
    Html(page(Rubrique::Journal, "Journal des consultations", &h, Some(&s))).into_response()
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
            interface: Arc::new(Interface::nouvelle(a, &conf, couts_fictifs(), 12)),
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
            "/copieurs/COPIEUR-1",
            "/services/Audit",
            "/cabinets/Est",
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
    async fn le_tableau_de_bord_montre_des_graphiques_concordants() {
        let b = banc();
        charger_jeu(&b);
        let cookie = b.connecter().await;
        let r = b.get(&format!("/?{NOVEMBRE}"), Some(&cookie)).await;
        // Le jeu tient sur un jour : 101 N&B + 60 couleur, soit les 161 pages de la tuile.
        assert!(r.texte.contains("<b>161</b>"), "{}", r.texte);
        assert!(
            r.texte.contains("01/11 : 161 pages · N&amp;B 101 · Couleur 60"),
            "{}",
            r.texte
        );
        assert!(r.texte.contains("class=\"graphique\"") && r.texte.contains("class=\"segments\""));
        assert!(r.texte.contains("Voir les chiffres"));
        assert!(
            !r.texte.contains("confidentiel"),
            "pas de nom de document dans un graphique"
        );
        // Une seule consultation au journal, malgré les deux lectures.
        assert_eq!(b.journal().iter().filter(|(_, a, _)| a == "tableau").count(), 1);

        let vide = b.get("/?du=2020-01-01&au=2020-01-31", Some(&cookie)).await;
        assert!(
            vide.texte.contains("Aucune page sur cette période."),
            "{}",
            vide.texte
        );
        assert!(!vide.texte.contains("class=\"graphique\""));
    }

    #[tokio::test]
    async fn periode_demesuree_refusee_sans_trace_de_lecture() {
        let b = banc();
        let cookie = b.connecter().await;
        let r = b.get("/?du=0001-01-01&au=9999-12-30", Some(&cookie)).await;
        assert_eq!(r.code, StatusCode::BAD_REQUEST);
        assert!(r.texte.contains("dix ans"), "{}", r.texte);
        assert!(!b.journal().iter().any(|(_, a, _)| a == "tableau"));
        let r = b.get("/?du=2020-01-01&au=2026-11-15", Some(&cookie)).await;
        assert_eq!(r.code, StatusCode::OK);
    }

    #[tokio::test]
    async fn page_des_copieurs_sans_releve_snmp() {
        let b = banc();
        charger_jeu(&b);
        {
            let mut base = b.base.lock().unwrap();
            let mut t = travail(9);
            t.file = "COPIEUR-3".into();
            t.utilisateur = "a.exemple".into();
            t.statut = tirage_modele::Statut::Erreur;
            t.envoye_ms = DEBUT_NOVEMBRE;
            base.enregistrer(&Evenement::Travail(t), 0).unwrap();
        }
        let cookie = b.connecter().await;
        let r = b.get(&format!("/copieurs?{NOVEMBRE}"), Some(&cookie)).await;
        assert_eq!(r.code, StatusCode::OK);
        assert!(
            r.texte.contains(&format!(
                "href=\"/copieurs/COPIEUR-1?{}\"",
                NOVEMBRE.replace('&', "&amp;")
            )),
            "{}",
            r.texte
        );
        // COPIEUR-1 : 151 pages, deux personnes hors fiche comprises, sans document.
        assert!(r.texte.contains(">151<"), "{}", r.texte);
        // Un copieur qui n'a fait qu'échouer reste visible.
        assert!(r.texte.contains("COPIEUR-3"));
        assert!(r.texte.contains("Aucun relevé"));
        assert!(!r.texte.contains("document-confidentiel"));
        assert!(b.journal().iter().any(|l| l.1 == "copieurs"));
    }

    #[tokio::test]
    async fn raccourcis_de_periode() {
        let b = banc();
        let cookie = b.connecter().await;
        // MAINTENANT = 15/11/2026.
        let r = b.get("/", Some(&cookie)).await;
        for attendu in [
            "href=\"/?du=2026-11-15&amp;au=2026-11-15\"",
            "href=\"/?du=2026-11-09&amp;au=2026-11-15\"",
            "href=\"/?du=2026-11-01&amp;au=2026-11-15\" aria-current",
            "href=\"/?du=2026-10-01&amp;au=2026-10-31\"",
            "href=\"/?du=2025-11-16&amp;au=2026-11-15\"",
        ] {
            assert!(r.texte.contains(attendu), "{attendu} {}", r.texte);
        }
    }

    #[tokio::test]
    async fn vues_filtrees_journalisees_et_sans_voisins() {
        let b = banc();
        charger_jeu(&b);
        let cookie = b.connecter().await;
        let r = b.get(&format!("/services/Audit?{NOVEMBRE}"), Some(&cookie)).await;
        assert_eq!(r.code, StatusCode::OK);
        assert!(
            r.texte.contains("a.exemple") && !r.texte.contains("b.exemple"),
            "{}",
            r.texte
        );
        assert!(r.texte.contains("<b>110</b>"), "{}", r.texte);
        assert!(!r.texte.contains("document-confidentiel"));
        let j = b.journal();
        assert!(
            j.iter()
                .any(|l| l.1 == "vue_service" && l.2.as_deref().is_some_and(|c| c.starts_with("Audit ")))
        );

        let r = b
            .get(&format!("/copieurs/COPIEUR-2?{NOVEMBRE}"), Some(&cookie))
            .await;
        assert!(
            r.texte.contains("<b>10</b>") && !r.texte.contains("b.exemple"),
            "{}",
            r.texte
        );

        let r = b.get(&format!("/cabinets/Est?{NOVEMBRE}"), Some(&cookie)).await;
        assert!(
            r.texte.contains("<b>160</b>") && r.texte.contains("Paie"),
            "{}",
            r.texte
        );

        // Personne sans fiche : le lien du rapport doit mener quelque part.
        let chemin = format!("/services/{}?{NOVEMBRE}", encoder_chemin("Non rattaché"));
        let r = b.get(&chemin, Some(&cookie)).await;
        assert!(
            r.texte.contains("c.exemple") && r.texte.contains("<b>1</b>"),
            "{}",
            r.texte
        );

        // Rien d'exécutable dans le titre ni dans l'action du formulaire.
        let chemin = format!("/copieurs/{}", encoder_chemin("<script>\"x"));
        let r = b.get(&chemin, Some(&cookie)).await;
        assert_eq!(r.code, StatusCode::OK);
        assert!(!r.texte.contains("<script>"), "{}", r.texte);

        // Période invalide : ni trace de lecture ni donnée.
        let avant = b.journal().len();
        let r = b.get("/services/Audit?du=x", Some(&cookie)).await;
        assert_eq!(r.code, StatusCode::BAD_REQUEST);
        assert_eq!(b.journal().len(), avant);
    }

    #[tokio::test]
    async fn fiche_d_une_personne_enrichie() {
        let b = banc();
        charger_jeu(&b);
        let cookie = b.connecter().await;
        let r = b.get("/utilisateurs/a.exemple", Some(&cookie)).await;
        assert!(r.texte.contains("<b>110</b>"), "{}", r.texte);
        assert!(r.texte.contains("COPIEUR-2"));
        assert!(r.texte.contains("document-confidentiel-fictif.pdf"));
        assert!(
            r.texte.contains(">Imprimé<") && r.texte.contains(">N&amp;B<"),
            "{}",
            r.texte
        );
        assert!(r.texte.contains("<svg"));
        assert!(
            r.texte.contains("du=2025-11-16&amp;au=2026-11-15\" aria-current"),
            "par défaut, l'onglet 12 mois : {}",
            r.texte
        );
        // Une période sans travail vide aussi la liste.
        let r = b
            .get(
                "/utilisateurs/a.exemple?du=2026-10-01&au=2026-10-02",
                Some(&cookie),
            )
            .await;
        assert!(!r.texte.contains("document-confidentiel"));
        assert!(
            r.texte
                .contains("Aucun travail pour cette personne sur cette période.")
        );
    }

    #[tokio::test]
    async fn comparaison_a_la_periode_precedente() {
        let b = banc();
        charger_jeu(&b);
        {
            let mut base = b.base.lock().unwrap();
            let mut t = travail(20);
            t.pages = 322;
            t.copies = 1;
            t.envoye_ms = DEBUT_NOVEMBRE - 86_400_000;
            base.enregistrer(&Evenement::Travail(t), 0).unwrap();
        }
        let cookie = b.connecter().await;
        // Du 01 au 02/11, comparé aux 30 et 31/10 : 161 pages contre 322.
        let r = b.get("/?du=2026-11-01&au=2026-11-02", Some(&cookie)).await;
        assert!(
            r.texte
                .contains("(du 30/10/2026 au 31/10/2026) : pages \u{2212}50 %"),
            "{}",
            r.texte
        );
        // Rien avant : pas de phrase.
        let r = b.get("/?du=2026-10-29&au=2026-10-30", Some(&cookie)).await;
        assert!(!r.texte.contains("période précédente"));
    }

    #[test]
    fn ecarts_arrondis_et_signes() {
        assert_eq!(ecart(10, 0), None);
        assert_eq!(ecart(100, 100).as_deref(), Some("stable"));
        assert_eq!(ecart(112, 100).as_deref(), Some("+12 %"));
        assert_eq!(ecart(50, 100).as_deref(), Some("\u{2212}50 %"));
        assert_eq!(ecart(2, 3).as_deref(), Some("\u{2212}33 %"));
        assert_eq!(ecart(0, 3).as_deref(), Some("\u{2212}100 %"));
    }

    #[test]
    fn durees_et_tailles_lisibles() {
        assert_eq!(duree(-5), "0 s");
        assert_eq!(duree(2_400), "2 s");
        assert_eq!(duree(125_000), "2 min 5 s");
        assert_eq!(duree(3_660_000), "1 h 1 min");
        assert_eq!(duree(7_200_000), "2 h");
        assert_eq!(taille(10), "10 o");
        assert_eq!(taille(2_048), "2 Ko");
        assert_eq!(taille(1_572_864), "1,5 Mo");
    }

    /// Jeu pour B1-B4 : COPIEUR-1 partagé par trois personnes, COPIEUR-2 par une seule.
    fn charger_partage(b: &Banc, recu_ms: i64) {
        let mut base = b.base.lock().unwrap();
        let mut ajouter = |n: u32, u: &str, f: &str, pages: u32, app: Option<&str>| {
            let mut t = travail(n);
            t.utilisateur = u.into();
            t.file = f.into();
            t.pages = pages;
            t.copies = 1;
            t.application = app.map(str::to_owned);
            t.envoye_ms = DEBUT_NOVEMBRE + i64::from(n);
            base.enregistrer(&Evenement::Travail(t), recu_ms).unwrap();
        };
        ajouter(1, "a.exemple", "COPIEUR-1", 250, Some("Tableur"));
        ajouter(2, "b.exemple", "COPIEUR-1", 10, Some("Tableur"));
        ajouter(3, "c.exemple", "COPIEUR-1", 10, None);
        ajouter(4, "a.exemple", "COPIEUR-2", 300, Some("Texte"));
    }

    #[tokio::test]
    async fn etat_de_la_collecte() {
        let b = banc();
        let cookie = b.connecter().await;
        let r = b.get("/", Some(&cookie)).await;
        assert!(
            r.texte.contains("aucun travail reçu depuis 30 jours"),
            "{}",
            r.texte
        );
        // MAINTENANT = dimanche 15/11 12:00 UTC ; reçu le vendredi 13/11 à 16:30 UTC, 17:30 à Paris : 30 min ouvrées.
        charger_partage(&b, MAINTENANT - 156_600_000);
        for chemin in ["/", "/copieurs"] {
            let r = b.get(chemin, Some(&cookie)).await;
            assert!(
                r.texte.contains("dernier travail reçu le 13/11/2026 17:30"),
                "{}",
                r.texte
            );
            assert!(!r.texte.contains("Collecte muette"));
        }
    }

    #[tokio::test]
    async fn collecte_muette_signalee() {
        let b = banc();
        // Mercredi 11/11 à 09:00 UTC : plus de 2 h ouvrées depuis.
        charger_partage(&b, MAINTENANT - 356_400_000);
        let cookie = b.connecter().await;
        let r = b.get("/", Some(&cookie)).await;
        assert!(r.texte.contains("Collecte muette"), "{}", r.texte);
        assert!(r.texte.contains("class=\"erreur\""));
    }

    #[tokio::test]
    async fn activite_horaire_seulement_global_et_copieur_partage() {
        let b = banc();
        charger_partage(&b, MAINTENANT);
        let cookie = b.connecter().await;
        let titre = "Activité par jour et par heure";
        let r = b.get(&format!("/?{NOVEMBRE}"), Some(&cookie)).await;
        assert!(
            r.texte.contains(titre) && r.texte.contains("chaleur n5"),
            "{}",
            r.texte
        );
        // Copieur partagé : affichée.
        let r = b
            .get(&format!("/copieurs/COPIEUR-1?{NOVEMBRE}"), Some(&cookie))
            .await;
        assert!(r.texte.contains(titre), "{}", r.texte);
        // Copieur d'une seule personne : masquée, comme tout ce qui la décrirait.
        let r = b
            .get(&format!("/copieurs/COPIEUR-2?{NOVEMBRE}"), Some(&cookie))
            .await;
        assert!(
            !r.texte.contains(titre) && !r.texte.contains("chaleur"),
            "{}",
            r.texte
        );
        // Jamais pour une personne ni pour un service.
        for chemin in ["/utilisateurs/a.exemple", "/services/Non%20rattach%C3%A9"] {
            let r = b.get(&format!("{chemin}?{NOVEMBRE}"), Some(&cookie)).await;
            assert_eq!(r.code, StatusCode::OK);
            assert!(
                !r.texte.contains(titre) && !r.texte.contains("chaleur"),
                "{chemin}"
            );
        }
    }

    #[tokio::test]
    async fn gros_travaux_par_copieur_seulement() {
        let b = banc();
        charger_partage(&b, MAINTENANT);
        let cookie = b.connecter().await;
        let r = b
            .get(&format!("/copieurs/COPIEUR-1?{NOVEMBRE}"), Some(&cookie))
            .await;
        assert!(
            r.texte
                .contains("Travaux d&#39;au moins 100 pages : 1, soit 250 pages et 93 % des pages"),
            "{}",
            r.texte
        );
        // 300 pages, mais une seule personne : aucun chiffre.
        let r = b
            .get(&format!("/copieurs/COPIEUR-2?{NOVEMBRE}"), Some(&cookie))
            .await;
        assert!(!r.texte.contains("Travaux d&#39;au moins"), "{}", r.texte);
        let r = b.get(&format!("/copieurs?{NOVEMBRE}"), Some(&cookie)).await;
        assert!(
            r.texte.contains("Gros travaux") && r.texte.contains(">—<"),
            "{}",
            r.texte
        );
        for chemin in ["/utilisateurs/a.exemple", "/palmares", "/rapports/utilisateur"] {
            let r = b.get(&format!("{chemin}?{NOVEMBRE}"), Some(&cookie)).await;
            assert!(!r.texte.contains("Gros travaux"), "{chemin}");
        }
    }

    #[tokio::test]
    async fn rapport_par_application_sans_poste() {
        let b = banc();
        charger_partage(&b, MAINTENANT);
        let cookie = b.connecter().await;
        let r = b
            .get(&format!("/rapports/application?{NOVEMBRE}"), Some(&cookie))
            .await;
        assert_eq!(r.code, StatusCode::OK);
        assert!(
            r.texte.contains("Tableur") && r.texte.contains("Inconnue"),
            "{}",
            r.texte
        );
        assert!(!r.texte.contains("a.exemple"), "pas de personne dans ce rapport");
        let r = b
            .get(&format!("/rapports/application/csv?{NOVEMBRE}"), Some(&cookie))
            .await;
        assert!(r.texte.contains("Tableur"));
        assert_eq!(
            b.get("/rapports/poste", Some(&cookie)).await.code,
            StatusCode::NOT_FOUND
        );
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

    #[tokio::test]
    async fn apres_la_purge_les_rapports_gardent_les_totaux_sans_nom() {
        let b = banc();
        const AN_DERNIER: i64 = MAINTENANT - 400 * 86_400_000; // 2025-10-11
        {
            let mut base = b.base.lock().unwrap();
            let mut t = travail(1);
            t.utilisateur = "a.exemple".into();
            t.envoye_ms = AN_DERNIER;
            t.nom_document = Some("document-confidentiel-fictif.pdf".into());
            base.enregistrer(&Evenement::Travail(t), 0).unwrap();
            base.personne_trouvee("a.exemple", Some("A Exemple"), Some("Audit"), None, 0)
                .unwrap();
            let limite = MAINTENANT - 365 * 86_400_000;
            base.purger(limite, limite, |ms| {
                crate::conservation::mois_de(ms, &TimeZone::UTC)
            })
            .unwrap();
        }
        let cookie = b.connecter().await;

        // Mois entier : le total reste, par service, avec l'avis.
        let r = b
            .get("/rapports/service?du=2025-10-01&au=2025-10-31", Some(&cookie))
            .await;
        assert!(
            r.texte.contains("Audit") && r.texte.contains("<td class=\"n\">6</td>"),
            "{}",
            r.texte
        );
        assert!(r.texte.contains("Le détail est effacé au-delà de 12 mois"));

        // Morceau de mois : les totaux mensuels ne se découpent pas.
        let r = b
            .get("/rapports/service?du=2025-10-05&au=2025-10-20", Some(&cookie))
            .await;
        assert!(!r.texte.contains("Audit"));

        // Plus personne par utilisateur, ni palmarès, ni détail.
        for chemin in [
            "/rapports/utilisateur?du=2025-10-01&au=2025-10-31",
            "/rapports/utilisateur/csv?du=2025-10-01&au=2025-10-31",
            "/palmares?du=2025-10-01&au=2025-10-31",
            "/utilisateurs/a.exemple",
        ] {
            let r = b.get(chemin, Some(&cookie)).await;
            assert_eq!(r.code, StatusCode::OK);
            assert!(!r.texte.contains("confidentiel"), "{chemin}");
            assert!(!r.texte.contains("A Exemple"), "{chemin}");
            if !chemin.starts_with("/utilisateurs") {
                assert!(!r.texte.contains("a.exemple"), "{chemin}");
            }
        }

        // Période récente : pas d'avis.
        let r = b.get("/", Some(&cookie)).await;
        assert!(!r.texte.contains("Le détail est effacé"));
    }

    #[test]
    fn mois_entiers_d_une_periode() {
        let z = TimeZone::UTC;
        let p = |du: &str, au: &str| {
            let d = Demande {
                du: Some(du.into()),
                au: Some(au.into()),
            };
            Periode::lire(&d, MAINTENANT, &z).unwrap().mois_entiers()
        };
        let m = |a: &str, b: &str| Some((a.to_owned(), b.to_owned()));
        assert_eq!(p("2025-10-01", "2025-10-31"), m("2025-10", "2025-10"));
        assert_eq!(p("2025-10-02", "2025-12-30"), m("2025-11", "2025-11"));
        assert_eq!(p("2024-02-01", "2024-02-29"), m("2024-02", "2024-02"));
        assert_eq!(p("2025-10-05", "2025-10-20"), None);
        assert_eq!(p("2025-10-05", "2025-11-20"), None);
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
