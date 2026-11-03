//! Base SQLite : schéma versionné, enregistrement idempotent des événements.
//!
//! Un travail est identifié par sa clé ([`Travail::cle`]) : le recevoir deux fois (accusé perdu, renvoi) ne crée
//! jamais de doublon. Même principe pour un relevé (collecteur, copieur, instant).

use std::path::Path;

use rusqlite::{Connection, params};
use tirage_modele::{Evenement, ReleveCopieur, Travail};

/// Migrations, dans l'ordre ; la n-ième amène la base à la version n (`PRAGMA user_version`).
const MIGRATIONS: &[&str] = &[
    // 1 — travaux et relevés
    "CREATE TABLE travaux (
        id            INTEGER PRIMARY KEY,
        cle           TEXT NOT NULL UNIQUE,
        collecteur    TEXT NOT NULL,
        numero        INTEGER NOT NULL,
        file          TEXT NOT NULL,
        pilote        TEXT,
        utilisateur   TEXT NOT NULL,
        poste         TEXT,
        envoye_ms     INTEGER NOT NULL,
        imprime_ms    INTEGER,
        pages         INTEGER NOT NULL,
        copies        INTEGER NOT NULL,
        feuilles      INTEGER NOT NULL,
        pages_totales INTEGER NOT NULL,
        couleur       TEXT NOT NULL,
        recto_verso   TEXT NOT NULL,
        format        TEXT,
        orientation   TEXT,
        resolution_ppp INTEGER,
        taille_octets INTEGER,
        type_donnees  TEXT,
        statut        TEXT NOT NULL,
        nom_document  TEXT,
        application   TEXT,
        recu_ms       INTEGER NOT NULL
     );
     CREATE INDEX travaux_envoye ON travaux(envoye_ms);
     CREATE INDEX travaux_utilisateur ON travaux(utilisateur, envoye_ms);
     CREATE INDEX travaux_file ON travaux(file, envoye_ms);
     CREATE TABLE releves (
        id          INTEGER PRIMARY KEY,
        collecteur  TEXT NOT NULL,
        copieur     TEXT NOT NULL,
        releve_ms   INTEGER NOT NULL,
        contenu     TEXT NOT NULL,
        recu_ms     INTEGER NOT NULL,
        UNIQUE (collecteur, copieur, releve_ms)
     );",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Issue {
    Nouveau,
    /// Déjà reçu : rien n'est réécrit.
    Doublon,
}

pub struct Base {
    conn: Connection,
}

/// Nom d'une variante d'énumération tel qu'en JSON (« noir_et_blanc »…).
fn nom<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

impl Base {
    pub fn ouvrir(chemin: &Path) -> rusqlite::Result<Self> {
        Base::preparer(Connection::open(chemin)?)
    }

    pub fn en_memoire() -> rusqlite::Result<Self> {
        Base::preparer(Connection::open_in_memory()?)
    }

    fn preparer(conn: Connection) -> rusqlite::Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut b = Base { conn };
        b.migrer()?;
        Ok(b)
    }

    pub fn version(&self) -> rusqlite::Result<usize> {
        self.conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .map(|v| v as usize)
    }

    fn migrer(&mut self) -> rusqlite::Result<()> {
        let actuelle = self.version()?;
        if actuelle > MIGRATIONS.len() {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "base en version {actuelle}, plus récente que ce serveur ({})",
                MIGRATIONS.len()
            )));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(actuelle) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn enregistrer(&mut self, e: &Evenement, recu_ms: i64) -> rusqlite::Result<Issue> {
        let n = match e {
            Evenement::Travail(t) => self.travail(t, recu_ms)?,
            Evenement::Releve(r) => self.releve(r, recu_ms)?,
        };
        Ok(if n == 1 { Issue::Nouveau } else { Issue::Doublon })
    }

    fn travail(&self, t: &Travail, recu_ms: i64) -> rusqlite::Result<usize> {
        self.conn.execute(
            "INSERT INTO travaux (cle, collecteur, numero, file, pilote, utilisateur, poste, envoye_ms, imprime_ms,
                pages, copies, feuilles, pages_totales, couleur, recto_verso, format, orientation, resolution_ppp,
                taille_octets, type_donnees, statut, nom_document, application, recu_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                ?21, ?22, ?23, ?24)
             ON CONFLICT (cle) DO NOTHING",
            params![
                t.cle(),
                t.collecteur,
                t.numero,
                t.file,
                t.pilote,
                t.utilisateur,
                t.poste,
                t.envoye_ms,
                t.imprime_ms,
                t.pages,
                t.copies.max(1),
                t.feuilles_estimees() as i64,
                t.pages_totales() as i64,
                nom(&t.couleur),
                nom(&t.recto_verso),
                t.format,
                t.orientation.as_ref().map(nom),
                t.resolution_ppp,
                t.taille_octets.map(|o| o as i64),
                t.type_donnees,
                nom(&t.statut),
                t.nom_document,
                t.application,
                recu_ms,
            ],
        )
    }

    fn releve(&self, r: &ReleveCopieur, recu_ms: i64) -> rusqlite::Result<usize> {
        let contenu = serde_json::to_string(r).unwrap_or_default();
        self.conn.execute(
            "INSERT INTO releves (collecteur, copieur, releve_ms, contenu, recu_ms) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (collecteur, copieur, releve_ms) DO NOTHING",
            params![r.collecteur, r.copieur, r.releve_ms, contenu, recu_ms],
        )
    }

    pub fn nombre_travaux(&self) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT count(*) FROM travaux", [], |r| r.get(0))
    }

    pub fn connexion(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tirage_modele::{ModeCouleur, Orientation, RectoVerso, Statut};

    pub fn travail(numero: u32) -> Travail {
        Travail {
            collecteur: "impression-1".into(),
            numero,
            file: "COPIEUR-EXEMPLE-1".into(),
            pilote: Some("Pilote EXEMPLE".into()),
            utilisateur: "a.exemple".into(),
            poste: Some("POSTE-EXEMPLE-01".into()),
            envoye_ms: 1_767_225_600_000 + i64::from(numero),
            imprime_ms: None,
            pages: 3,
            copies: 2,
            feuilles: None,
            couleur: ModeCouleur::NoirEtBlanc,
            recto_verso: RectoVerso::BordLong,
            format: Some("A4".into()),
            orientation: Some(Orientation::Paysage),
            resolution_ppp: Some(600),
            taille_octets: Some(1000),
            type_donnees: Some("RAW".into()),
            statut: Statut::Imprime,
            nom_document: None,
            application: Some("PDF".into()),
        }
    }

    #[test]
    fn schema_et_idempotence() {
        let mut b = Base::en_memoire().unwrap();
        assert_eq!(b.version().unwrap(), MIGRATIONS.len());
        let e = Evenement::Travail(travail(1));
        assert_eq!(b.enregistrer(&e, 10).unwrap(), Issue::Nouveau);
        assert_eq!(b.enregistrer(&e, 20).unwrap(), Issue::Doublon);
        assert_eq!(b.nombre_travaux().unwrap(), 1);
        let (feuilles, total, couleur, orientation): (i64, i64, String, String) = b
            .connexion()
            .query_row(
                "SELECT feuilles, pages_totales, couleur, orientation FROM travaux",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (feuilles, total, couleur.as_str(), orientation.as_str()),
            (4, 6, "noir_et_blanc", "paysage")
        );
    }

    #[test]
    fn reouverture_sans_rejouer_les_migrations() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.sqlite");
        Base::ouvrir(&p)
            .unwrap()
            .enregistrer(&Evenement::Travail(travail(1)), 0)
            .unwrap();
        let b = Base::ouvrir(&p).unwrap();
        assert_eq!(b.nombre_travaux().unwrap(), 1);
    }

    #[test]
    fn base_plus_recente_refusee() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.sqlite");
        Connection::open(&p)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(Base::ouvrir(&p).is_err());
    }

    #[test]
    fn releve_idempotent() {
        let mut b = Base::en_memoire().unwrap();
        let r = Evenement::Releve(ReleveCopieur {
            collecteur: "impression-1".into(),
            copieur: "COPIEUR-EXEMPLE-1".into(),
            releve_ms: 5,
            modele: None,
            numero_serie: None,
            compteur_total: Some(1),
            compteur_noir: None,
            compteur_couleur: None,
            consommables: vec![],
            etat: None,
            alertes: vec![],
        });
        assert_eq!(b.enregistrer(&r, 0).unwrap(), Issue::Nouveau);
        assert_eq!(b.enregistrer(&r, 0).unwrap(), Issue::Doublon);
    }
}
