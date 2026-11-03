//! API de réception des collecteurs.
//!
//! `POST /api/v1/evenements` (jeton du collecteur en `Authorization: Bearer`) :
//! `{"evenements": [{"id": 1, "enveloppe": {…}}, …]}` → `{"acquittes": [1, …], "refuses": [{"id": 2, "raison": "…"}]}`.
//!
//! - Un événement déjà reçu est **acquitté** (le collecteur peut le retirer de sa file) : c'est l'idempotence.
//! - Un événement illisible, d'une version inconnue ou d'un autre collecteur que celui du jeton est **refusé** :
//!   le collecteur le met de côté, il ne bloque pas la suite.
//! - Une erreur de la base renvoie 503 : rien n'est acquitté, le collecteur réessaiera.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tirage_modele::{Enveloppe, Evenement};

use crate::base::Base;
use crate::config::Config;

/// Taille maximale d'un lot (événements) et d'un corps de requête.
pub const LOT_MAX: usize = 500;
pub const CORPS_MAX: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct Etat {
    pub config: Arc<Config>,
    pub base: Arc<Mutex<Base>>,
    pub horloge: fn() -> i64,
}

#[derive(Debug, Deserialize)]
pub struct Lot {
    pub evenements: Vec<Element>,
}

#[derive(Debug, Deserialize)]
pub struct Element {
    pub id: u64,
    /// Gardé brut : la version est vérifiée avant d'interpréter le contenu.
    pub enveloppe: Box<serde_json::value::RawValue>,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Reponse {
    pub acquittes: Vec<u64>,
    pub refuses: Vec<Refus>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Refus {
    pub id: u64,
    pub raison: String,
}

pub fn routeur(etat: Etat) -> Router {
    Router::new()
        .route("/sante", get(|| async { "ok" }))
        .route("/api/v1/evenements", post(recevoir))
        .layer(DefaultBodyLimit::max(CORPS_MAX))
        .with_state(etat)
}

fn erreur(code: StatusCode, message: &str) -> Response {
    (code, message.to_owned()).into_response()
}

fn collecteur_du_jeton<'a>(config: &'a Config, entetes: &HeaderMap) -> Option<&'a str> {
    let v = entetes.get(header::AUTHORIZATION)?.to_str().ok()?;
    let jeton = v.strip_prefix("Bearer ")?.trim();
    if jeton.is_empty() {
        return None;
    }
    config.authentifier(jeton)
}

fn collecteur_de(e: &Evenement) -> &str {
    match e {
        Evenement::Travail(t) => &t.collecteur,
        Evenement::Releve(r) => &r.collecteur,
    }
}

async fn recevoir(State(etat): State<Etat>, entetes: HeaderMap, corps: axum::body::Bytes) -> Response {
    let Some(collecteur) = collecteur_du_jeton(&etat.config, &entetes).map(str::to_owned) else {
        return erreur(StatusCode::UNAUTHORIZED, "jeton absent ou inconnu");
    };
    let lot: Lot = match serde_json::from_slice(&corps) {
        Ok(l) => l,
        Err(e) => return erreur(StatusCode::BAD_REQUEST, &format!("lot illisible : {e}")),
    };
    if lot.evenements.len() > LOT_MAX {
        return erreur(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!("au plus {LOT_MAX} événements par lot"),
        );
    }
    let maintenant = (etat.horloge)();
    let base = Arc::clone(&etat.base);
    let resultat = tokio::task::spawn_blocking(move || {
        let mut base = base.lock().map_err(|_| "base indisponible".to_owned())?;
        let mut r = Reponse::default();
        for el in lot.evenements {
            let env = match Enveloppe::decoder(el.enveloppe.get().as_bytes()) {
                Ok(env) => env,
                Err(e) => {
                    r.refuses.push(Refus {
                        id: el.id,
                        raison: e.to_string(),
                    });
                    continue;
                }
            };
            if collecteur_de(&env.evenement) != collecteur {
                r.refuses.push(Refus {
                    id: el.id,
                    raison: "l'événement ne vient pas du collecteur de ce jeton".into(),
                });
                continue;
            }
            base.enregistrer(&env.evenement, maintenant)
                .map_err(|e| e.to_string())?;
            r.acquittes.push(el.id);
        }
        Ok::<_, String>(r)
    })
    .await;
    match resultat {
        Ok(Ok(r)) => (StatusCode::OK, axum::Json(r)).into_response(),
        Ok(Err(e)) => {
            eprintln!("erreur de la base : {e}");
            erreur(
                StatusCode::SERVICE_UNAVAILABLE,
                "base indisponible, réessayer plus tard",
            )
        }
        Err(_) => erreur(StatusCode::INTERNAL_SERVER_ERROR, "erreur interne"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::tests::travail;
    use crate::config::empreinte;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const JETON: &str = "jeton-essai";

    fn etat() -> Etat {
        let config = Config::lire(&format!(
            "ecoute = \"127.0.0.1:0\"\nbase = \"x\"\n[[collecteurs]]\nnom = \"impression-1\"\njeton_sha256 = \"{}\"\n",
            empreinte(JETON)
        ))
        .unwrap();
        Etat {
            config: Arc::new(config),
            base: Arc::new(Mutex::new(Base::en_memoire().unwrap())),
            horloge: || 42,
        }
    }

    async fn envoyer(etat: &Etat, jeton: Option<&str>, corps: String) -> (StatusCode, String) {
        let mut req = Request::post("/api/v1/evenements").header(header::CONTENT_TYPE, "application/json");
        if let Some(j) = jeton {
            req = req.header(header::AUTHORIZATION, format!("Bearer {j}"));
        }
        let rep = routeur(etat.clone())
            .oneshot(req.body(Body::from(corps)).unwrap())
            .await
            .unwrap();
        let code = rep.status();
        let octets = rep.into_body().collect().await.unwrap().to_bytes();
        (code, String::from_utf8_lossy(&octets).into_owned())
    }

    fn lot(elements: &[(u64, String)]) -> String {
        let v: Vec<String> = elements
            .iter()
            .map(|(id, env)| format!("{{\"id\":{id},\"enveloppe\":{env}}}"))
            .collect();
        format!("{{\"evenements\":[{}]}}", v.join(","))
    }

    fn env_travail(numero: u32) -> String {
        String::from_utf8(Enveloppe::nouvelle(Evenement::Travail(travail(numero))).encoder()).unwrap()
    }

    #[tokio::test]
    async fn lot_recu_et_renvoi_idempotent() {
        let e = etat();
        let corps = lot(&[(1, env_travail(1)), (2, env_travail(2))]);
        let (code, texte) = envoyer(&e, Some(JETON), corps.clone()).await;
        assert_eq!(code, StatusCode::OK, "{texte}");
        let r: Reponse = serde_json::from_str(&texte).unwrap();
        assert_eq!(r.acquittes, vec![1, 2]);
        // Même lot renvoyé (accusé perdu) : acquitté, sans doublon en base.
        let (_, texte) = envoyer(&e, Some(JETON), corps).await;
        assert_eq!(
            serde_json::from_str::<Reponse>(&texte).unwrap().acquittes,
            vec![1, 2]
        );
        assert_eq!(e.base.lock().unwrap().nombre_travaux().unwrap(), 2);
    }

    #[tokio::test]
    async fn jeton_obligatoire() {
        let e = etat();
        assert_eq!(envoyer(&e, None, lot(&[])).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(
            envoyer(&e, Some("faux"), lot(&[])).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn refus_par_element_sans_bloquer_le_lot() {
        let e = etat();
        let autre = env_travail(3).replace("impression-1", "impression-2");
        let corps = lot(&[
            (1, r#"{"version":99,"evenement":{}}"#.into()),
            (2, autre),
            (3, r#"{"version":1,"evenement":{"type":"inconnu"}}"#.into()),
            (4, env_travail(4)),
        ]);
        let (code, texte) = envoyer(&e, Some(JETON), corps).await;
        assert_eq!(code, StatusCode::OK);
        let r: Reponse = serde_json::from_str(&texte).unwrap();
        assert_eq!(r.acquittes, vec![4]);
        assert_eq!(r.refuses.iter().map(|x| x.id).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(r.refuses[0].raison.contains("version"));
        assert!(r.refuses[1].raison.contains("collecteur"));
    }

    #[tokio::test]
    async fn lot_illisible_ou_trop_gros() {
        let e = etat();
        assert_eq!(
            envoyer(&e, Some(JETON), "pas du json".into()).await.0,
            StatusCode::BAD_REQUEST
        );
        let gros: Vec<(u64, String)> = (0..=LOT_MAX as u64).map(|i| (i, "{}".to_owned())).collect();
        assert_eq!(
            envoyer(&e, Some(JETON), lot(&gros)).await.0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[tokio::test]
    async fn sante() {
        let rep = routeur(etat())
            .oneshot(Request::get("/sante").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(rep.status(), StatusCode::OK);
    }
}
