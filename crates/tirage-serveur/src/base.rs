//! Base SQLite : schéma versionné, enregistrement idempotent des événements.
//!
//! Un travail est identifié par sa clé ([`Travail::cle`]) : le recevoir deux fois (accusé perdu, renvoi) ne crée
//! jamais de doublon. Même principe pour un relevé (collecteur, copieur, instant).

use std::path::Path;

use crate::config::{ConfigCouts, TarifFile};
use rusqlite::{Connection, OptionalExtension, params};
use tirage_modele::{Evenement, ReleveCopieur, Travail};

use crate::rapports::Axe;

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
    // 2 — fiches de l'annuaire (seulement les personnes qui ont imprimé) et journal des consultations
    "CREATE TABLE personnes (
        utilisateur TEXT PRIMARY KEY COLLATE NOCASE,
        nom         TEXT,
        service     TEXT,
        cabinet     TEXT,
        trouve      INTEGER NOT NULL,
        verifie_ms  INTEGER NOT NULL
     );
     CREATE TABLE consultations (
        id       INTEGER PRIMARY KEY,
        quand_ms INTEGER NOT NULL,
        lecteur  TEXT NOT NULL,
        action   TEXT NOT NULL,
        cible    TEXT
     );
     CREATE INDEX consultations_quand ON consultations(quand_ms);
     CREATE TRIGGER consultations_immuables BEFORE UPDATE ON consultations
     BEGIN
        SELECT RAISE(ABORT, 'le journal des consultations ne se modifie pas');
     END;",
    // 3 — totaux mensuels sans nom, seuls survivants du détail après la durée de conservation
    "CREATE TABLE totaux_mensuels (
        mois        TEXT NOT NULL,
        service     TEXT NOT NULL,
        cabinet     TEXT NOT NULL,
        file        TEXT NOT NULL,
        couleur     TEXT NOT NULL,
        recto_verso TEXT NOT NULL,
        format      TEXT NOT NULL,
        travaux     INTEGER NOT NULL,
        pages       INTEGER NOT NULL,
        feuilles    INTEGER NOT NULL,
        PRIMARY KEY (mois, service, cabinet, file, couleur, recto_verso, format)
     );",
    // 4 — prix saisis dans l'interface ; la ligne de file vide porte le tarif général et prime sur la configuration
    "CREATE TABLE tarifs (
        file    TEXT PRIMARY KEY COLLATE NOCASE,
        noir    REAL,
        couleur REAL
     );",
];

/// Ce qu'une purge a effacé.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BilanPurge {
    pub travaux: usize,
    pub personnes: usize,
    pub consultations: usize,
}

/// Fiche de l'annuaire rattachée à un utilisateur vu dans les travaux.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Personne {
    pub nom: Option<String>,
    pub service: Option<String>,
    pub cabinet: Option<String>,
    /// Encore présent dans l'annuaire au dernier passage ; sinon, dernières valeurs connues.
    pub trouve: bool,
}

/// Une ligne du journal des consultations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consultation {
    pub quand_ms: i64,
    pub lecteur: String,
    pub action: String,
    pub cible: Option<String>,
}

/// Agrégat brut d'un rapport : une ligne par (clé, file, mode couleur, recto-verso).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LigneBrute {
    pub cle: String,
    pub nom: Option<String>,
    pub file: String,
    pub couleur: String,
    pub recto_verso: String,
    pub travaux: i64,
    pub pages: i64,
    pub feuilles: i64,
}

/// Un travail, tel que l'affiche le détail par utilisateur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LigneTravail {
    pub envoye_ms: i64,
    pub file: String,
    pub poste: Option<String>,
    pub pages: i64,
    pub copies: i64,
    pub feuilles: i64,
    pub couleur: String,
    pub recto_verso: String,
    pub format: Option<String>,
    pub statut: String,
    pub nom_document: Option<String>,
    pub application: Option<String>,
    pub imprime_ms: Option<i64>,
    pub taille_octets: Option<i64>,
    pub resolution_ppp: Option<i64>,
    pub orientation: Option<String>,
}

/// Restreint une lecture à un service, un cabinet, un copieur ou une personne.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filtre<'a> {
    Tout,
    Service(&'a str),
    Cabinet(&'a str),
    Copieur(&'a str),
    Utilisateur(&'a str),
}

impl<'a> Filtre<'a> {
    /// Condition SQL, sur `travaux t` joint à `personnes p`, et sa valeur en paramètre `?3`.
    fn condition(self) -> (&'static str, Option<&'a str>) {
        match self {
            Filtre::Tout => ("?3 IS NULL", None),
            Filtre::Service(v) => (
                "COALESCE(NULLIF(p.service, ''), 'Non rattaché') = ?3 COLLATE NOCASE",
                Some(v),
            ),
            Filtre::Cabinet(v) => (
                "COALESCE(NULLIF(p.cabinet, ''), 'Non rattaché') = ?3 COLLATE NOCASE",
                Some(v),
            ),
            Filtre::Copieur(v) => ("t.file = ?3 COLLATE NOCASE", Some(v)),
            Filtre::Utilisateur(v) => ("t.utilisateur = ?3 COLLATE NOCASE", Some(v)),
        }
    }
}

/// Activité d'un copieur sur une période, tous statuts confondus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiviteCopieur {
    pub file: String,
    pub dernier_ms: i64,
    pub non_imprimes: i64,
    pub personnes: i64,
    /// Travaux couleur imprimés d'au moins le seuil demandé, et leurs pages.
    pub gros_travaux: i64,
    pub gros_pages: i64,
}

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
        // Ce qui est effacé est écrasé dans le fichier, pas seulement marqué libre.
        conn.pragma_update(None, "secure_delete", "ON")?;
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

    /// Travaux imprimés de la période `[du_ms, au_ms[`, regroupés selon l'axe.
    pub fn agreger(&self, axe: Axe, du_ms: i64, au_ms: i64) -> rusqlite::Result<Vec<LigneBrute>> {
        self.agreger_filtre(axe, du_ms, au_ms, Filtre::Tout)
    }

    /// Comme `agreger`, restreint par `filtre`.
    pub fn agreger_filtre(
        &self,
        axe: Axe,
        du_ms: i64,
        au_ms: i64,
        filtre: Filtre<'_>,
    ) -> rusqlite::Result<Vec<LigneBrute>> {
        let (condition, valeur) = filtre.condition();
        let cle = match axe {
            Axe::Utilisateur => "t.utilisateur",
            Axe::Service => "COALESCE(NULLIF(p.service, ''), 'Non rattaché')",
            Axe::Cabinet => "COALESCE(NULLIF(p.cabinet, ''), 'Non rattaché')",
            Axe::Copieur => "t.file",
            Axe::Format => "COALESCE(NULLIF(t.format, ''), 'Inconnu')",
            Axe::Application => "COALESCE(NULLIF(t.application, ''), 'Inconnue')",
        };
        let sql = format!(
            "SELECT {cle}, max(p.nom), t.file, t.couleur, t.recto_verso, count(*), sum(t.pages_totales), sum(t.feuilles)
             FROM travaux t LEFT JOIN personnes p ON t.utilisateur = p.utilisateur COLLATE NOCASE
             WHERE t.envoye_ms >= ?1 AND t.envoye_ms < ?2 AND t.statut = 'imprime' AND {condition}
             GROUP BY 1, 3, 4, 5"
        );
        let mut s = self.conn.prepare(&sql)?;
        s.query_map(params![du_ms, au_ms, valeur], |r| {
            Ok(LigneBrute {
                cle: r.get(0)?,
                nom: if axe == Axe::Utilisateur { r.get(1)? } else { None },
                file: r.get(2)?,
                couleur: r.get(3)?,
                recto_verso: r.get(4)?,
                travaux: r.get(5)?,
                pages: r.get(6)?,
                feuilles: r.get(7)?,
            })
        })?
        .collect()
    }

    /// Pages imprimées de `[du_ms, au_ms[` par quart d'heure et mode couleur : l'appelant les range dans son fuseau, ce
    /// que SQLite ne sait pas faire aux changements d'heure.
    pub fn pages_dans_le_temps(&self, du_ms: i64, au_ms: i64) -> rusqlite::Result<Vec<(i64, String, i64)>> {
        self.pages_dans_le_temps_filtre(du_ms, au_ms, Filtre::Tout)
    }

    /// Comme `pages_dans_le_temps`, restreint par `filtre`.
    pub fn pages_dans_le_temps_filtre(
        &self,
        du_ms: i64,
        au_ms: i64,
        filtre: Filtre<'_>,
    ) -> rusqlite::Result<Vec<(i64, String, i64)>> {
        let (condition, valeur) = filtre.condition();
        let mut s = self.conn.prepare(&format!(
            "SELECT t.envoye_ms / 900000 * 900000, t.couleur, sum(t.pages_totales)
             FROM travaux t LEFT JOIN personnes p ON t.utilisateur = p.utilisateur COLLATE NOCASE
             WHERE t.envoye_ms >= ?1 AND t.envoye_ms < ?2 AND t.statut = 'imprime' AND {condition}
             GROUP BY 1, 2"
        ))?;
        s.query_map(params![du_ms, au_ms, valeur], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect()
    }

    /// Dernière réception de chaque collecteur, parmi les travaux envoyés depuis `depuis_ms`. Un collecteur retiré
    /// finit par sortir de la liste.
    pub fn dernieres_receptions(&self, depuis_ms: i64) -> rusqlite::Result<Vec<(String, i64)>> {
        let mut s = self.conn.prepare(
            "SELECT collecteur, max(recu_ms) FROM travaux WHERE envoye_ms >= ?1 GROUP BY collecteur ORDER BY collecteur",
        )?;
        s.query_map([depuis_ms], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    }

    /// Dernier travail, travaux non imprimés et nombre de personnes de chaque copieur, sur `[du_ms, au_ms[`.
    pub fn activite_copieurs(
        &self,
        du_ms: i64,
        au_ms: i64,
        seuil_couleur: i64,
    ) -> rusqlite::Result<Vec<ActiviteCopieur>> {
        let mut s = self.conn.prepare(
            "SELECT file, max(envoye_ms), sum(statut <> 'imprime'), count(DISTINCT utilisateur COLLATE NOCASE),
                sum(statut = 'imprime' AND couleur = 'couleur' AND pages_totales >= ?3),
                sum(CASE WHEN statut = 'imprime' AND couleur = 'couleur' AND pages_totales >= ?3
                    THEN pages_totales ELSE 0 END)
             FROM travaux WHERE envoye_ms >= ?1 AND envoye_ms < ?2
             GROUP BY file COLLATE NOCASE ORDER BY file",
        )?;
        s.query_map([du_ms, au_ms, seuil_couleur], |r| {
            Ok(ActiviteCopieur {
                file: r.get(0)?,
                dernier_ms: r.get(1)?,
                non_imprimes: r.get(2)?,
                personnes: r.get(3)?,
                gros_travaux: r.get(4)?,
                gros_pages: r.get(5)?,
            })
        })?
        .collect()
    }

    /// Tarifs en vigueur : ceux de l'interface s'ils ont été enregistrés une fois, sinon ceux de la configuration.
    pub fn tarifs(&self, defaut: &ConfigCouts) -> rusqlite::Result<ConfigCouts> {
        let mut s = self
            .conn
            .prepare("SELECT file, noir, couleur FROM tarifs ORDER BY file COLLATE NOCASE")?;
        let lignes = s
            .query_map([], |r| {
                Ok(TarifFile {
                    file: r.get(0)?,
                    noir: r.get(1)?,
                    couleur: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let Some((general, files)) = lignes.split_first().filter(|(g, _)| g.file.is_empty()) else {
            return Ok(defaut.clone());
        };
        Ok(ConfigCouts {
            devise: defaut.devise.clone(),
            noir: general.noir,
            couleur: general.couleur,
            files: files.to_vec(),
        })
    }

    /// Remplace tous les tarifs et l'inscrit au journal, d'un bloc : sans trace, pas de modification.
    pub fn remplacer_tarifs(
        &self,
        c: &ConfigCouts,
        quand_ms: i64,
        lecteur: &str,
        cible: &str,
    ) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        self.consigner(quand_ms, lecteur, "tarifs_modifies", Some(cible))?;
        tx.execute("DELETE FROM tarifs", [])?;
        let mut s = tx.prepare("INSERT INTO tarifs (file, noir, couleur) VALUES (?1, ?2, ?3)")?;
        s.execute(params!["", c.noir, c.couleur])?;
        for f in &c.files {
            s.execute(params![f.file, f.noir, f.couleur])?;
        }
        drop(s);
        tx.commit()
    }

    /// Files vues dans le détail, les totaux archivés ou les tarifs, une seule fois quelle que soit la casse.
    pub fn files_connues(&self) -> rusqlite::Result<Vec<String>> {
        let mut s = self.conn.prepare(
            "SELECT file FROM travaux UNION SELECT file FROM totaux_mensuels
             UNION SELECT file FROM tarifs WHERE file <> ''
             ORDER BY 1 COLLATE NOCASE",
        )?;
        let mut files = s
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        files.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        Ok(files)
    }

    /// Pages archivées des mois `[premier, dernier]` (« AAAA-MM »), par mois et mode couleur.
    pub fn pages_archivees(
        &self,
        premier: &str,
        dernier: &str,
    ) -> rusqlite::Result<Vec<(String, String, i64)>> {
        let mut s = self.conn.prepare(
            "SELECT mois, couleur, sum(pages) FROM totaux_mensuels WHERE mois >= ?1 AND mois <= ?2 GROUP BY 1, 2",
        )?;
        s.query_map([premier, dernier], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect()
    }

    /// Totaux archivés des mois `[premier, dernier]` (« AAAA-MM »), selon l'axe. Aucun par utilisateur : ils sont sans nom.
    pub fn totaux_archives(
        &self,
        axe: Axe,
        premier: &str,
        dernier: &str,
    ) -> rusqlite::Result<Vec<LigneBrute>> {
        let cle = match axe {
            // Ni personne ni application dans les totaux archivés.
            Axe::Utilisateur | Axe::Application => return Ok(Vec::new()),
            Axe::Service => "service",
            Axe::Cabinet => "cabinet",
            Axe::Copieur => "file",
            Axe::Format => "format",
        };
        let sql = format!(
            "SELECT {cle}, file, couleur, recto_verso, sum(travaux), sum(pages), sum(feuilles)
             FROM totaux_mensuels WHERE mois >= ?1 AND mois <= ?2 GROUP BY 1, 2, 3, 4"
        );
        let mut s = self.conn.prepare(&sql)?;
        s.query_map([premier, dernier], |r| {
            Ok(LigneBrute {
                cle: r.get(0)?,
                nom: None,
                file: r.get(1)?,
                couleur: r.get(2)?,
                recto_verso: r.get(3)?,
                travaux: r.get(4)?,
                pages: r.get(5)?,
                feuilles: r.get(6)?,
            })
        })?
        .collect()
    }

    /// Passe en totaux sans nom les travaux d'avant `limite_ms`, efface leur détail et les fiches devenues sans objet,
    /// purge le journal d'avant `limite_journal_ms`. Tout ou rien : une coupure ne perd ni ne double aucun total.
    pub fn purger(
        &mut self,
        limite_ms: i64,
        limite_journal_ms: i64,
        mois_de: impl Fn(i64) -> String,
    ) -> rusqlite::Result<BilanPurge> {
        let tx = self.conn.transaction()?;
        {
            let mut lire = tx.prepare(
                "SELECT t.envoye_ms, COALESCE(NULLIF(p.service, ''), 'Non rattaché'),
                    COALESCE(NULLIF(p.cabinet, ''), 'Non rattaché'), t.file, t.couleur, t.recto_verso,
                    COALESCE(NULLIF(t.format, ''), 'Inconnu'), t.pages_totales, t.feuilles
                 FROM travaux t LEFT JOIN personnes p ON t.utilisateur = p.utilisateur COLLATE NOCASE
                 WHERE t.envoye_ms < ?1 AND t.statut = 'imprime'",
            )?;
            let mut ajouter = tx.prepare(
                "INSERT INTO totaux_mensuels (mois, service, cabinet, file, couleur, recto_verso, format, travaux,
                    pages, feuilles)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9)
                 ON CONFLICT (mois, service, cabinet, file, couleur, recto_verso, format) DO UPDATE SET
                    travaux = travaux + 1, pages = pages + excluded.pages, feuilles = feuilles + excluded.feuilles",
            )?;
            let mut lignes = lire.query([limite_ms])?;
            while let Some(r) = lignes.next()? {
                ajouter.execute(params![
                    mois_de(r.get(0)?),
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                ])?;
            }
        }
        let travaux = tx.execute("DELETE FROM travaux WHERE envoye_ms < ?1", [limite_ms])?;
        let personnes = tx.execute(
            "DELETE FROM personnes WHERE NOT EXISTS (
                SELECT 1 FROM travaux t WHERE t.utilisateur = personnes.utilisateur COLLATE NOCASE)",
            [],
        )?;
        let consultations = tx.execute(
            "DELETE FROM consultations WHERE quand_ms < ?1",
            [limite_journal_ms],
        )?;
        tx.commit()?;
        // Vide le journal d'écriture, qui garde sinon une copie des pages effacées.
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(BilanPurge {
            travaux,
            personnes,
            consultations,
        })
    }

    /// Copie cohérente vers un fichier neuf ou vide.
    pub fn copier_vers(&self, destination: &Path) -> rusqlite::Result<()> {
        self.conn
            .execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])?;
        Ok(())
    }

    /// Travaux de la période non comptés (annulés, en erreur, statut inconnu).
    pub fn non_imprimes(&self, du_ms: i64, au_ms: i64) -> rusqlite::Result<i64> {
        self.conn.query_row(
            "SELECT count(*) FROM travaux WHERE envoye_ms >= ?1 AND envoye_ms < ?2 AND statut <> 'imprime'",
            [du_ms, au_ms],
            |r| r.get(0),
        )
    }

    /// Dernier relevé de chaque copieur. Un relevé illisible est ignoré.
    pub fn derniers_releves(&self) -> rusqlite::Result<Vec<ReleveCopieur>> {
        let mut s = self.conn.prepare(
            "SELECT contenu, max(releve_ms) FROM releves GROUP BY collecteur, copieur ORDER BY copieur",
        )?;
        let contenus: Vec<String> = s.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(contenus
            .iter()
            .filter_map(|c| serde_json::from_str(c).ok())
            .collect())
    }

    /// Utilisateurs vus dans les travaux, sans fiche ou dont la fiche date d'avant `avant_ms`.
    pub fn utilisateurs_a_verifier(&self, avant_ms: i64) -> rusqlite::Result<Vec<String>> {
        let mut s = self.conn.prepare(
            "SELECT DISTINCT t.utilisateur COLLATE NOCASE
             FROM travaux t LEFT JOIN personnes p ON t.utilisateur = p.utilisateur COLLATE NOCASE
             WHERE p.utilisateur IS NULL OR p.verifie_ms < ?1",
        )?;
        s.query_map([avant_ms], |r| r.get(0))?.collect()
    }

    /// Fiche relue dans l'annuaire.
    pub fn personne_trouvee(
        &self,
        utilisateur: &str,
        nom: Option<&str>,
        service: Option<&str>,
        cabinet: Option<&str>,
        ms: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO personnes (utilisateur, nom, service, cabinet, trouve, verifie_ms)
             VALUES (?1, ?2, ?3, ?4, 1, ?5)
             ON CONFLICT (utilisateur) DO UPDATE SET nom = excluded.nom, service = excluded.service,
                cabinet = excluded.cabinet, trouve = 1, verifie_ms = excluded.verifie_ms",
            params![utilisateur, nom, service, cabinet, ms],
        )?;
        Ok(())
    }

    /// Absent de l'annuaire (départ, compte supprimé) : les dernières valeurs connues restent, pour l'historique.
    pub fn personne_absente(&self, utilisateur: &str, ms: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO personnes (utilisateur, trouve, verifie_ms) VALUES (?1, 0, ?2)
             ON CONFLICT (utilisateur) DO UPDATE SET trouve = 0, verifie_ms = excluded.verifie_ms",
            params![utilisateur, ms],
        )?;
        Ok(())
    }

    pub fn personne(&self, utilisateur: &str) -> rusqlite::Result<Option<Personne>> {
        self.conn
            .query_row(
                "SELECT nom, service, cabinet, trouve FROM personnes WHERE utilisateur = ?1",
                [utilisateur],
                |r| {
                    Ok(Personne {
                        nom: r.get(0)?,
                        service: r.get(1)?,
                        cabinet: r.get(2)?,
                        trouve: r.get(3)?,
                    })
                },
            )
            .optional()
    }

    /// Ajoute une ligne au journal des consultations.
    pub fn consigner(
        &self,
        quand_ms: i64,
        lecteur: &str,
        action: &str,
        cible: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO consultations (quand_ms, lecteur, action, cible) VALUES (?1, ?2, ?3, ?4)",
            params![quand_ms, lecteur, action, cible],
        )?;
        Ok(())
    }

    /// Dernières lignes du journal, la plus récente d'abord.
    pub fn consultations(&self, max: u32) -> rusqlite::Result<Vec<Consultation>> {
        let mut s = self.conn.prepare(
            "SELECT quand_ms, lecteur, action, cible FROM consultations ORDER BY id DESC LIMIT ?1",
        )?;
        s.query_map([max], |r| {
            Ok(Consultation {
                quand_ms: r.get(0)?,
                lecteur: r.get(1)?,
                action: r.get(2)?,
                cible: r.get(3)?,
            })
        })?
        .collect()
    }

    /// Derniers travaux d'un utilisateur sur `[du_ms, au_ms[`, le plus récent d'abord.
    pub fn travaux_de(
        &self,
        utilisateur: &str,
        du_ms: i64,
        au_ms: i64,
        max: u32,
    ) -> rusqlite::Result<Vec<LigneTravail>> {
        let mut s = self.conn.prepare(
            "SELECT envoye_ms, file, poste, pages, copies, feuilles, couleur, recto_verso, format, statut,
                nom_document, application, imprime_ms, taille_octets, resolution_ppp, orientation
             FROM travaux WHERE utilisateur = ?1 COLLATE NOCASE AND envoye_ms >= ?2 AND envoye_ms < ?3
             ORDER BY envoye_ms DESC LIMIT ?4",
        )?;
        s.query_map(params![utilisateur, du_ms, au_ms, max], |r| {
            Ok(LigneTravail {
                envoye_ms: r.get(0)?,
                file: r.get(1)?,
                poste: r.get(2)?,
                pages: r.get(3)?,
                copies: r.get(4)?,
                feuilles: r.get(5)?,
                couleur: r.get(6)?,
                recto_verso: r.get(7)?,
                format: r.get(8)?,
                statut: r.get(9)?,
                nom_document: r.get(10)?,
                application: r.get(11)?,
                imprime_ms: r.get(12)?,
                taille_octets: r.get(13)?,
                resolution_ppp: r.get(14)?,
                orientation: r.get(15)?,
            })
        })?
        .collect()
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

    #[test]
    fn migration_depuis_la_version_1_garde_les_travaux() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.sqlite");
        {
            let c = Connection::open(&p).unwrap();
            c.execute_batch(MIGRATIONS[0]).unwrap();
            c.pragma_update(None, "user_version", 1).unwrap();
            c.execute(
                "INSERT INTO travaux (cle, collecteur, numero, file, utilisateur, envoye_ms, pages, copies, feuilles,
                    pages_totales, couleur, recto_verso, statut, recu_ms)
                 VALUES ('k', 'c', 1, 'f', 'u', 0, 1, 1, 1, 1, 'couleur', 'recto', 'imprime', 0)",
                [],
            )
            .unwrap();
        }
        let b = Base::ouvrir(&p).unwrap();
        assert_eq!(b.version().unwrap(), MIGRATIONS.len());
        assert_eq!(b.nombre_travaux().unwrap(), 1);
        assert_eq!(b.utilisateurs_a_verifier(0).unwrap(), vec!["u".to_owned()]);
    }

    #[test]
    fn fiche_absente_garde_les_dernieres_valeurs() {
        let b = Base::en_memoire().unwrap();
        b.personne_trouvee("a.exemple", Some("A Exemple"), Some("Audit"), None, 10)
            .unwrap();
        b.personne_absente("A.EXEMPLE", 20).unwrap();
        let p = b.personne("a.exemple").unwrap().unwrap();
        assert_eq!(p.service.as_deref(), Some("Audit"));
        assert!(!p.trouve);
        b.personne_absente("b.exemple", 20).unwrap();
        assert_eq!(b.personne("b.exemple").unwrap().unwrap().nom, None);
        assert_eq!(b.personne("c.exemple").unwrap(), None);
    }

    #[test]
    fn a_verifier_selon_age_et_casse() {
        let mut b = Base::en_memoire().unwrap();
        b.enregistrer(&Evenement::Travail(travail(1)), 0).unwrap();
        let mut t2 = travail(2);
        t2.utilisateur = "A.Exemple".into();
        b.enregistrer(&Evenement::Travail(t2), 0).unwrap();
        assert_eq!(b.utilisateurs_a_verifier(100).unwrap().len(), 1);
        b.personne_trouvee("a.exemple", None, None, None, 50).unwrap();
        assert!(b.utilisateurs_a_verifier(50).unwrap().is_empty());
        assert_eq!(b.utilisateurs_a_verifier(51).unwrap().len(), 1);
    }

    #[test]
    fn journal_non_modifiable() {
        let b = Base::en_memoire().unwrap();
        b.consigner(1, "lecteur", "detail_utilisateur", Some("a.exemple"))
            .unwrap();
        b.consigner(2, "lecteur", "journal", None).unwrap();
        assert!(
            b.connexion()
                .execute("UPDATE consultations SET lecteur = 'x'", [])
                .is_err()
        );
        let j = b.consultations(10).unwrap();
        assert_eq!((j[0].quand_ms, j[1].lecteur.as_str()), (2, "lecteur"));
    }

    #[test]
    fn travaux_du_seul_utilisateur_demande() {
        let mut b = Base::en_memoire().unwrap();
        b.enregistrer(&Evenement::Travail(travail(1)), 0).unwrap();
        b.enregistrer(&Evenement::Travail(travail(2)), 0).unwrap();
        let mut autre = travail(3);
        autre.utilisateur = "b.exemple".into();
        b.enregistrer(&Evenement::Travail(autre), 0).unwrap();
        let l = b.travaux_de("A.EXEMPLE", 0, i64::MAX, 10).unwrap();
        assert_eq!(l.len(), 2);
        assert!(l[0].envoye_ms > l[1].envoye_ms);
        assert_eq!(b.travaux_de("a.exemple", 0, i64::MAX, 1).unwrap().len(), 1);
        // La fin de la période est exclue.
        let dernier = l[0].envoye_ms;
        let avant = b.travaux_de("a.exemple", 0, dernier, 10).unwrap();
        assert_eq!(avant.len(), 1);
        assert_eq!(avant[0].envoye_ms, l[1].envoye_ms);
    }

    #[test]
    fn un_filtre_ne_rend_que_ce_qu_il_vise() {
        let mut b = Base::en_memoire().unwrap();
        let mut ajouter = |n: u32, u: &str, f: &str| {
            let mut t = travail(n);
            t.utilisateur = u.into();
            t.file = f.into();
            b.enregistrer(&Evenement::Travail(t), 0).unwrap();
        };
        ajouter(1, "a.exemple", "COPIEUR-1");
        ajouter(2, "b.exemple", "COPIEUR-1");
        ajouter(3, "c.exemple", "COPIEUR-2");
        b.personne_trouvee("a.exemple", None, Some("Audit"), Some("Est"), 0)
            .unwrap();
        b.personne_trouvee("b.exemple", None, Some("Paie"), Some("Est"), 0)
            .unwrap();
        let cles = |f| -> Vec<String> {
            let mut v: Vec<String> = b
                .agreger_filtre(Axe::Utilisateur, 0, i64::MAX, f)
                .unwrap()
                .into_iter()
                .map(|l| l.cle)
                .collect();
            v.sort();
            v
        };
        assert_eq!(cles(Filtre::Tout).len(), 3);
        assert_eq!(cles(Filtre::Service("AUDIT")), ["a.exemple"]);
        assert_eq!(cles(Filtre::Service("Non rattaché")), ["c.exemple"]);
        assert_eq!(cles(Filtre::Cabinet("Est")), ["a.exemple", "b.exemple"]);
        assert_eq!(cles(Filtre::Copieur("copieur-2")), ["c.exemple"]);
        assert_eq!(cles(Filtre::Utilisateur("B.EXEMPLE")), ["b.exemple"]);
        assert!(
            cles(Filtre::Service("' OR 1=1 --")).is_empty(),
            "valeur en paramètre"
        );
        let temps = b
            .pages_dans_le_temps_filtre(0, i64::MAX, Filtre::Copieur("COPIEUR-2"))
            .unwrap();
        let tout = b.pages_dans_le_temps(0, i64::MAX).unwrap();
        let somme = |v: &[(i64, String, i64)]| v.iter().map(|l| l.2).sum::<i64>();
        assert!(somme(&temps) > 0 && somme(&temps) < somme(&tout));
    }

    #[test]
    fn activite_des_copieurs_tous_statuts() {
        let mut b = Base::en_memoire().unwrap();
        let mut ajouter = |n: u32, u: &str, statut, ms: i64| {
            let mut t = travail(n);
            t.utilisateur = u.into();
            t.statut = statut;
            t.envoye_ms = ms;
            t.couleur = ModeCouleur::Couleur;
            b.enregistrer(&Evenement::Travail(t), 0).unwrap();
        };
        ajouter(1, "a.exemple", Statut::Imprime, 10);
        ajouter(2, "A.EXEMPLE", Statut::Erreur, 30);
        ajouter(3, "b.exemple", Statut::Annule, 20);
        let a = b.activite_copieurs(0, 100, 1).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].dernier_ms, a[0].non_imprimes, a[0].personnes), (30, 2, 2));
        // Seul le travail imprimé compte comme gros.
        assert_eq!(a[0].gros_travaux, 1);
        assert!(a[0].gros_pages > 0);
        assert_eq!(b.activite_copieurs(0, 100, 1_000_000).unwrap()[0].gros_travaux, 0);
        assert!(b.activite_copieurs(0, 10, 1).unwrap().is_empty());
    }

    #[test]
    fn un_gros_travail_est_forcement_en_couleur() {
        let mut b = Base::en_memoire().unwrap();
        let mut t = travail(1);
        t.couleur = ModeCouleur::NoirEtBlanc;
        t.pages = 500;
        t.envoye_ms = 10;
        b.enregistrer(&Evenement::Travail(t), 0).unwrap();
        let a = b.activite_copieurs(0, 100, 1).unwrap();
        assert_eq!((a[0].gros_travaux, a[0].gros_pages), (0, 0));
    }

    #[test]
    fn tarifs_de_l_interface_ou_de_la_configuration() {
        let b = Base::en_memoire().unwrap();
        let defaut = ConfigCouts {
            noir: Some(0.5),
            ..ConfigCouts::default()
        };
        assert_eq!(b.tarifs(&defaut).unwrap(), defaut);
        let saisis = ConfigCouts {
            devise: defaut.devise.clone(),
            noir: None,
            couleur: Some(0.1),
            files: vec![TarifFile {
                file: "F-1".into(),
                noir: Some(0.02),
                couleur: None,
            }],
        };
        b.remplacer_tarifs(&saisis, 1, "lecteur", "x").unwrap();
        // Une fois enregistrés, ils priment, même vides : le prix noir de la configuration ne revient pas.
        assert_eq!(b.tarifs(&defaut).unwrap(), saisis);
        assert_eq!(b.tarifs(&defaut).unwrap().prix("f-1", false), Some(2000));
        assert_eq!(b.tarifs(&defaut).unwrap().prix("f-1", true), Some(10_000));
        assert_eq!(b.tarifs(&defaut).unwrap().prix("autre", false), None);
        assert_eq!(b.consultations(1).unwrap()[0].action, "tarifs_modifies");
    }

    #[test]
    fn sans_journal_les_tarifs_ne_changent_pas() {
        let b = Base::en_memoire().unwrap();
        b.conn.execute_batch("DROP TABLE consultations").unwrap();
        let defaut = ConfigCouts::default();
        let saisis = ConfigCouts {
            noir: Some(0.1),
            ..ConfigCouts::default()
        };
        assert!(b.remplacer_tarifs(&saisis, 1, "lecteur", "x").is_err());
        assert_eq!(b.tarifs(&defaut).unwrap(), defaut);
    }

    #[test]
    fn files_connues_sans_doublon_de_casse() {
        let mut b = Base::en_memoire().unwrap();
        for (n, f) in [(1, "COPIEUR"), (2, "copieur"), (3, "autre")] {
            let mut t = travail(n);
            t.file = f.into();
            b.enregistrer(&Evenement::Travail(t), 0).unwrap();
        }
        let tarif = ConfigCouts {
            files: vec![TarifFile {
                file: "retire".into(),
                noir: Some(0.1),
                couleur: None,
            }],
            ..ConfigCouts::default()
        };
        b.remplacer_tarifs(&tarif, 1, "lecteur", "x").unwrap();
        let f = b.files_connues().unwrap();
        assert_eq!(f.len(), 3, "{f:?}");
        assert!(
            f[0].eq_ignore_ascii_case("autre") && f[1].eq_ignore_ascii_case("copieur") && f[2] == "retire"
        );
    }

    #[test]
    fn dernieres_receptions_par_collecteur() {
        let mut b = Base::en_memoire().unwrap();
        let mut ajouter = |n: u32, c: &str, envoye: i64, recu: i64| {
            let mut t = travail(n);
            t.collecteur = c.into();
            t.envoye_ms = envoye;
            b.enregistrer(&Evenement::Travail(t), recu).unwrap();
        };
        ajouter(1, "un", 100, 110);
        ajouter(2, "un", 200, 250);
        ajouter(3, "deux", 300, 300);
        ajouter(4, "ancien", 10, 20);
        assert_eq!(
            b.dernieres_receptions(50).unwrap(),
            [("deux".to_owned(), 300), ("un".to_owned(), 250)]
        );
    }

    #[test]
    fn agregat_par_application() {
        let mut b = Base::en_memoire().unwrap();
        let mut t = travail(1);
        t.application = Some("Tableur".into());
        b.enregistrer(&Evenement::Travail(t), 0).unwrap();
        let mut t = travail(2);
        t.application = None;
        b.enregistrer(&Evenement::Travail(t), 0).unwrap();
        let mut cles: Vec<String> = b
            .agreger(Axe::Application, 0, i64::MAX)
            .unwrap()
            .into_iter()
            .map(|l| l.cle)
            .collect();
        cles.sort();
        assert_eq!(cles, ["Inconnue", "Tableur"]);
        assert!(
            b.totaux_archives(Axe::Application, "2026-01", "2026-12")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn agregats_par_axe_avec_rattachement() {
        let mut b = Base::en_memoire().unwrap();
        let mut t1 = travail(1);
        t1.utilisateur = "A.Exemple".into();
        b.enregistrer(&Evenement::Travail(t1), 0).unwrap();
        b.enregistrer(&Evenement::Travail(travail(2)), 0).unwrap();
        let mut annule = travail(3);
        annule.statut = tirage_modele::Statut::Annule;
        b.enregistrer(&Evenement::Travail(annule), 0).unwrap();
        let mut inconnu = travail(4);
        inconnu.utilisateur = "b.exemple".into();
        inconnu.format = None;
        b.enregistrer(&Evenement::Travail(inconnu), 0).unwrap();
        b.personne_trouvee("a.exemple", Some("A Exemple"), Some("Audit"), None, 0)
            .unwrap();

        let (du, au) = (0, i64::MAX);
        let s = b.agreger(Axe::Service, du, au).unwrap();
        let pages = |nom: &str| -> i64 { s.iter().filter(|l| l.cle == nom).map(|l| l.pages).sum() };
        assert_eq!((pages("Audit"), pages("Non rattaché")), (12, 6), "annulé exclu");
        let f = b.agreger(Axe::Format, du, au).unwrap();
        assert!(f.iter().any(|l| l.cle == "Inconnu"));
        let u = b.agreger(Axe::Utilisateur, du, au).unwrap();
        assert!(u.iter().any(|l| l.nom.as_deref() == Some("A Exemple")));
        assert_eq!(b.non_imprimes(du, au).unwrap(), 1);
        // Borne haute exclue.
        let debut = travail(1).envoye_ms;
        assert!(b.agreger(Axe::Copieur, 0, debut).unwrap().is_empty());
        assert_eq!(b.agreger(Axe::Copieur, debut, debut + 1).unwrap()[0].travaux, 1);
    }

    #[test]
    fn dernier_releve_de_chaque_copieur() {
        let mut b = Base::en_memoire().unwrap();
        for (copieur, ms, total) in [("C1", 1, 10), ("C1", 3, 30), ("C1", 2, 20), ("C2", 1, 5)] {
            let r = Evenement::Releve(ReleveCopieur {
                collecteur: "impression-1".into(),
                copieur: copieur.into(),
                releve_ms: ms,
                modele: None,
                numero_serie: None,
                compteur_total: Some(total),
                compteur_noir: None,
                compteur_couleur: None,
                consommables: vec![],
                etat: None,
                alertes: vec![],
            });
            b.enregistrer(&r, 0).unwrap();
        }
        let l = b.derniers_releves().unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].copieur.as_str(), l[0].compteur_total), ("C1", Some(30)));
    }
}
