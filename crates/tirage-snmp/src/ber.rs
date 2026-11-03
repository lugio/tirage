//! Codage BER du sous-ensemble de SNMP v2c utile en lecture (RFC 3416).
//!
//! Message : `SEQUENCE { version INTEGER, communauté OCTET STRING, PDU }`, la PDU portant l'identifiant de requête,
//! l'état d'erreur, l'index d'erreur et la liste des couples (OID, valeur).

use std::fmt;
use std::str::FromStr;

const T_INTEGER: u8 = 0x02;
const T_OCTETS: u8 = 0x04;
const T_NULL: u8 = 0x05;
const T_OID: u8 = 0x06;
const T_SEQUENCE: u8 = 0x30;
const T_IP: u8 = 0x40;
const T_COUNTER32: u8 = 0x41;
const T_GAUGE32: u8 = 0x42;
const T_TIMETICKS: u8 = 0x43;
const T_OPAQUE: u8 = 0x44;
const T_COUNTER64: u8 = 0x46;
const T_NO_SUCH_OBJECT: u8 = 0x80;
const T_NO_SUCH_INSTANCE: u8 = 0x81;
const T_END_OF_MIB: u8 = 0x82;

/// Version SNMP v2c dans le message.
pub const VERSION_V2C: i64 = 1;

/// Identifiant d'objet, ex. `1.3.6.1.2.1.1.1.0`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Oid(pub Vec<u32>);

impl Oid {
    /// `self` est-il sous `base` (ou égal) ?
    pub fn sous(&self, base: &Oid) -> bool {
        self.0.starts_with(&base.0)
    }

    /// Ajoute des composants (ex. un index de table).
    pub fn plus(&self, suite: &[u32]) -> Oid {
        let mut v = self.0.clone();
        v.extend_from_slice(suite);
        Oid(v)
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut premier = true;
        for c in &self.0 {
            if !premier {
                f.write_str(".")?;
            }
            write!(f, "{c}")?;
            premier = false;
        }
        Ok(())
    }
}

impl FromStr for Oid {
    type Err = ErreurBer;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim().trim_start_matches('.');
        let v: Result<Vec<u32>, _> = s.split('.').map(str::parse).collect();
        match v {
            Ok(v) if v.len() >= 2 && v[0] <= 2 && (v[0] == 2 || v[1] < 40) => Ok(Oid(v)),
            _ => Err(ErreurBer(format!("OID invalide : « {s} »"))),
        }
    }
}

/// Valeur d'un couple (OID, valeur).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Valeur {
    Entier(i64),
    Octets(Vec<u8>),
    Nul,
    Oid(Oid),
    Ip([u8; 4]),
    Compteur32(u32),
    Jauge32(u32),
    Ticks(u32),
    Opaque(Vec<u8>),
    Compteur64(u64),
    /// Exceptions v2c : l'objet ou l'instance n'existe pas, ou fin de la MIB (GETNEXT).
    PasDObjet,
    PasDInstance,
    FinDeMib,
}

impl Valeur {
    pub fn est_exception(&self) -> bool {
        matches!(self, Valeur::PasDObjet | Valeur::PasDInstance | Valeur::FinDeMib)
    }

    /// Valeur numérique, quel que soit le type entier SNMP.
    pub fn nombre(&self) -> Option<i64> {
        match *self {
            Valeur::Entier(n) => Some(n),
            Valeur::Compteur32(n) | Valeur::Jauge32(n) | Valeur::Ticks(n) => Some(i64::from(n)),
            Valeur::Compteur64(n) => i64::try_from(n).ok(),
            _ => None,
        }
    }

    /// Texte lisible d'une chaîne d'octets (UTF-8, à défaut Latin-1), sans les zéros de fin.
    pub fn texte(&self) -> Option<String> {
        match self {
            Valeur::Octets(o) => {
                let o = match o.iter().rposition(|&b| b != 0) {
                    Some(fin) => &o[..=fin],
                    None => &[][..],
                };
                let t = match std::str::from_utf8(o) {
                    Ok(t) => t.to_owned(),
                    Err(_) => o.iter().map(|&b| char::from(b)).collect(),
                };
                let t = t.trim().to_owned();
                (!t.is_empty()).then_some(t)
            }
            _ => None,
        }
    }
}

impl fmt::Display for Valeur {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Valeur::Entier(n) => write!(f, "INTEGER: {n}"),
            Valeur::Octets(o) => match self.texte() {
                Some(t) if o.iter().all(|&b| b == 0 || b >= 0x20 || b == b'\t') => write!(f, "STRING: {t:?}"),
                _ => {
                    f.write_str("Hex-STRING:")?;
                    for b in o {
                        write!(f, " {b:02X}")?;
                    }
                    Ok(())
                }
            },
            Valeur::Nul => f.write_str("NULL"),
            Valeur::Oid(o) => write!(f, "OID: {o}"),
            Valeur::Ip(a) => write!(f, "IpAddress: {}.{}.{}.{}", a[0], a[1], a[2], a[3]),
            Valeur::Compteur32(n) => write!(f, "Counter32: {n}"),
            Valeur::Jauge32(n) => write!(f, "Gauge32: {n}"),
            Valeur::Ticks(n) => write!(f, "Timeticks: {n}"),
            Valeur::Opaque(o) => write!(f, "Opaque: {} octet(s)", o.len()),
            Valeur::Compteur64(n) => write!(f, "Counter64: {n}"),
            Valeur::PasDObjet => f.write_str("noSuchObject"),
            Valeur::PasDInstance => f.write_str("noSuchInstance"),
            Valeur::FinDeMib => f.write_str("endOfMibView"),
        }
    }
}

/// Type de PDU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypePdu {
    Get,
    GetNext,
    Reponse,
}

impl TypePdu {
    fn etiquette(self) -> u8 {
        match self {
            TypePdu::Get => 0xA0,
            TypePdu::GetNext => 0xA1,
            TypePdu::Reponse => 0xA2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub version: i64,
    pub communaute: Vec<u8>,
    pub pdu: TypePdu,
    pub id_requete: i32,
    pub etat_erreur: i64,
    pub index_erreur: i64,
    pub couples: Vec<(Oid, Valeur)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErreurBer(pub String);

impl fmt::Display for ErreurBer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "message SNMP illisible : {}", self.0)
    }
}

impl std::error::Error for ErreurBer {}

fn err<T>(m: &str) -> Result<T, ErreurBer> {
    Err(ErreurBer(m.to_owned()))
}

// ---------------------------------------------------------------- codage

fn longueur(sortie: &mut Vec<u8>, n: usize) {
    if n < 0x80 {
        sortie.push(n as u8);
    } else {
        let octets: Vec<u8> = n.to_be_bytes().into_iter().skip_while(|&b| b == 0).collect();
        sortie.push(0x80 | octets.len() as u8);
        sortie.extend(octets);
    }
}

fn tlv(sortie: &mut Vec<u8>, etiquette: u8, contenu: &[u8]) {
    sortie.push(etiquette);
    longueur(sortie, contenu.len());
    sortie.extend_from_slice(contenu);
}

fn entier_signe(n: i64) -> Vec<u8> {
    let o = n.to_be_bytes();
    let mut debut = 0;
    // Retire les octets de tête redondants (complément à deux minimal).
    while debut < 7
        && ((o[debut] == 0x00 && o[debut + 1] & 0x80 == 0) || (o[debut] == 0xFF && o[debut + 1] & 0x80 != 0))
    {
        debut += 1;
    }
    o[debut..].to_vec()
}

fn entier_non_signe(n: u64) -> Vec<u8> {
    let mut v: Vec<u8> = n.to_be_bytes().into_iter().skip_while(|&b| b == 0).collect();
    if v.first().is_none_or(|&b| b & 0x80 != 0) {
        v.insert(0, 0);
    }
    v
}

fn oid_octets(oid: &Oid) -> Vec<u8> {
    let c = &oid.0;
    let mut v = Vec::new();
    let mut base128 = |mut n: u32| {
        let mut tmp = vec![(n & 0x7F) as u8];
        n >>= 7;
        while n > 0 {
            tmp.push(0x80 | (n & 0x7F) as u8);
            n >>= 7;
        }
        tmp.reverse();
        v.extend(tmp);
    };
    base128(c[0] * 40 + c.get(1).copied().unwrap_or(0));
    for &n in c.iter().skip(2) {
        base128(n);
    }
    v
}

fn valeur(sortie: &mut Vec<u8>, v: &Valeur) {
    match v {
        Valeur::Entier(n) => tlv(sortie, T_INTEGER, &entier_signe(*n)),
        Valeur::Octets(o) => tlv(sortie, T_OCTETS, o),
        Valeur::Nul => tlv(sortie, T_NULL, &[]),
        Valeur::Oid(o) => tlv(sortie, T_OID, &oid_octets(o)),
        Valeur::Ip(a) => tlv(sortie, T_IP, a),
        Valeur::Compteur32(n) => tlv(sortie, T_COUNTER32, &entier_non_signe(u64::from(*n))),
        Valeur::Jauge32(n) => tlv(sortie, T_GAUGE32, &entier_non_signe(u64::from(*n))),
        Valeur::Ticks(n) => tlv(sortie, T_TIMETICKS, &entier_non_signe(u64::from(*n))),
        Valeur::Opaque(o) => tlv(sortie, T_OPAQUE, o),
        Valeur::Compteur64(n) => tlv(sortie, T_COUNTER64, &entier_non_signe(*n)),
        Valeur::PasDObjet => tlv(sortie, T_NO_SUCH_OBJECT, &[]),
        Valeur::PasDInstance => tlv(sortie, T_NO_SUCH_INSTANCE, &[]),
        Valeur::FinDeMib => tlv(sortie, T_END_OF_MIB, &[]),
    }
}

impl Message {
    /// Requête v2c de lecture : les valeurs sont `NULL`.
    pub fn requete(communaute: &[u8], pdu: TypePdu, id_requete: i32, oids: &[Oid]) -> Self {
        Message {
            version: VERSION_V2C,
            communaute: communaute.to_vec(),
            pdu,
            id_requete,
            etat_erreur: 0,
            index_erreur: 0,
            couples: oids.iter().map(|o| (o.clone(), Valeur::Nul)).collect(),
        }
    }

    pub fn encoder(&self) -> Vec<u8> {
        let mut liste = Vec::new();
        for (oid, v) in &self.couples {
            let mut couple = Vec::new();
            tlv(&mut couple, T_OID, &oid_octets(oid));
            valeur(&mut couple, v);
            tlv(&mut liste, T_SEQUENCE, &couple);
        }
        let mut pdu = Vec::new();
        tlv(&mut pdu, T_INTEGER, &entier_signe(i64::from(self.id_requete)));
        tlv(&mut pdu, T_INTEGER, &entier_signe(self.etat_erreur));
        tlv(&mut pdu, T_INTEGER, &entier_signe(self.index_erreur));
        tlv(&mut pdu, T_SEQUENCE, &liste);
        let mut corps = Vec::new();
        tlv(&mut corps, T_INTEGER, &entier_signe(self.version));
        tlv(&mut corps, T_OCTETS, &self.communaute);
        tlv(&mut corps, self.pdu.etiquette(), &pdu);
        let mut sortie = Vec::new();
        tlv(&mut sortie, T_SEQUENCE, &corps);
        sortie
    }

    pub fn decoder(octets: &[u8]) -> Result<Self, ErreurBer> {
        let mut l = Lecteur(octets);
        let (t, corps) = l.tlv()?;
        if t != T_SEQUENCE {
            return err("le message n'est pas une séquence");
        }
        if !l.0.is_empty() {
            return err("octets en trop après le message");
        }
        let mut c = Lecteur(corps);
        let version = c.entier()?;
        let communaute = c.attendu(T_OCTETS)?.to_vec();
        let (etiquette, pdu) = c.tlv()?;
        let pdu_type = match etiquette {
            0xA0 => TypePdu::Get,
            0xA1 => TypePdu::GetNext,
            0xA2 => TypePdu::Reponse,
            e => return Err(ErreurBer(format!("type de PDU non pris en charge : 0x{e:02X}"))),
        };
        let mut p = Lecteur(pdu);
        let id = p.entier()?;
        let id_requete =
            i32::try_from(id).map_err(|_| ErreurBer("identifiant de requête hors limites".into()))?;
        let etat_erreur = p.entier()?;
        let index_erreur = p.entier()?;
        let mut liste = Lecteur(p.attendu(T_SEQUENCE)?);
        let mut couples = Vec::new();
        while !liste.0.is_empty() {
            let mut couple = Lecteur(liste.attendu(T_SEQUENCE)?);
            let oid = decoder_oid(couple.attendu(T_OID)?)?;
            let (t, contenu) = couple.tlv()?;
            couples.push((oid, decoder_valeur(t, contenu)?));
        }
        Ok(Message {
            version,
            communaute,
            pdu: pdu_type,
            id_requete,
            etat_erreur,
            index_erreur,
            couples,
        })
    }
}

// ---------------------------------------------------------------- décodage

struct Lecteur<'a>(&'a [u8]);

impl<'a> Lecteur<'a> {
    fn tlv(&mut self) -> Result<(u8, &'a [u8]), ErreurBer> {
        let d = self.0;
        if d.len() < 2 {
            return err("message tronqué");
        }
        let etiquette = d[0];
        let (n, entete) = if d[1] < 0x80 {
            (usize::from(d[1]), 2)
        } else {
            let k = usize::from(d[1] & 0x7F);
            if k == 0 || k > 4 || d.len() < 2 + k {
                return err("longueur invalide");
            }
            let n = d[2..2 + k]
                .iter()
                .fold(0usize, |acc, &b| (acc << 8) | usize::from(b));
            (n, 2 + k)
        };
        if d.len() < entete + n {
            return err("message tronqué");
        }
        self.0 = &d[entete + n..];
        Ok((etiquette, &d[entete..entete + n]))
    }

    fn attendu(&mut self, etiquette: u8) -> Result<&'a [u8], ErreurBer> {
        let (t, c) = self.tlv()?;
        if t != etiquette {
            return Err(ErreurBer(format!("type 0x{t:02X} au lieu de 0x{etiquette:02X}")));
        }
        Ok(c)
    }

    fn entier(&mut self) -> Result<i64, ErreurBer> {
        decoder_signe(self.attendu(T_INTEGER)?)
    }
}

fn decoder_signe(o: &[u8]) -> Result<i64, ErreurBer> {
    if o.is_empty() || o.len() > 8 {
        return err("entier de taille invalide");
    }
    let mut n: i64 = if o[0] & 0x80 != 0 { -1 } else { 0 };
    for &b in o {
        n = (n << 8) | i64::from(b);
    }
    Ok(n)
}

fn decoder_non_signe(o: &[u8], max_octets: usize) -> Result<u64, ErreurBer> {
    let o = match o {
        [0, reste @ ..] => reste,
        _ => o,
    };
    if o.is_empty() {
        return Ok(0);
    }
    if o.len() > max_octets {
        return err("compteur trop grand");
    }
    Ok(o.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b)))
}

fn decoder_oid(o: &[u8]) -> Result<Oid, ErreurBer> {
    let mut composants = Vec::new();
    let mut n: u64 = 0;
    for (i, &b) in o.iter().enumerate() {
        n = (n << 7) | u64::from(b & 0x7F);
        if n > u64::from(u32::MAX) {
            return err("composant d'OID trop grand");
        }
        if b & 0x80 == 0 {
            if composants.is_empty() {
                let (a, b) = if n < 40 {
                    (0, n)
                } else if n < 80 {
                    (1, n - 40)
                } else {
                    (2, n - 80)
                };
                composants.push(a as u32);
                composants.push(b as u32);
            } else {
                composants.push(n as u32);
            }
            n = 0;
        } else if i == o.len() - 1 {
            return err("OID tronqué");
        }
    }
    if composants.is_empty() {
        return err("OID vide");
    }
    Ok(Oid(composants))
}

fn decoder_valeur(t: u8, c: &[u8]) -> Result<Valeur, ErreurBer> {
    let u32_de = |c: &[u8]| decoder_non_signe(c, 4).map(|n| n as u32);
    Ok(match t {
        T_INTEGER => Valeur::Entier(decoder_signe(c)?),
        T_OCTETS => Valeur::Octets(c.to_vec()),
        T_NULL => Valeur::Nul,
        T_OID => Valeur::Oid(decoder_oid(c)?),
        T_IP => match c {
            [a, b, d, e] => Valeur::Ip([*a, *b, *d, *e]),
            _ => return err("adresse IP de taille invalide"),
        },
        T_COUNTER32 => Valeur::Compteur32(u32_de(c)?),
        T_GAUGE32 => Valeur::Jauge32(u32_de(c)?),
        T_TIMETICKS => Valeur::Ticks(u32_de(c)?),
        T_OPAQUE => Valeur::Opaque(c.to_vec()),
        T_COUNTER64 => Valeur::Compteur64(decoder_non_signe(c, 8)?),
        T_NO_SUCH_OBJECT => Valeur::PasDObjet,
        T_NO_SUCH_INSTANCE => Valeur::PasDInstance,
        T_END_OF_MIB => Valeur::FinDeMib,
        t => return Err(ErreurBer(format!("type de valeur inconnu : 0x{t:02X}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(s: &str) -> Oid {
        s.parse().unwrap()
    }

    #[test]
    fn requete_get_octet_pres() {
        // GET v2c « public », id 1, sysDescr.0 — octets calculés à la main d'après la RFC.
        let m = Message::requete(b"public", TypePdu::Get, 1, &[oid("1.3.6.1.2.1.1.1.0")]);
        let attendu: Vec<u8> = vec![
            0x30, 0x26, 0x02, 0x01, 0x01, 0x04, 0x06, b'p', b'u', b'b', b'l', b'i', b'c', 0xA0, 0x19, 0x02,
            0x01, 0x01, 0x02, 0x01, 0x00, 0x02, 0x01, 0x00, 0x30, 0x0E, 0x30, 0x0C, 0x06, 0x08, 0x2B, 0x06,
            0x01, 0x02, 0x01, 0x01, 0x01, 0x00, 0x05, 0x00,
        ];
        assert_eq!(m.encoder(), attendu);
        assert_eq!(Message::decoder(&attendu).unwrap(), m);
    }

    #[test]
    fn aller_retour_tous_les_types() {
        let m = Message {
            version: VERSION_V2C,
            communaute: b"exemple".to_vec(),
            pdu: TypePdu::Reponse,
            id_requete: -123_456,
            etat_erreur: 0,
            index_erreur: 0,
            couples: vec![
                (
                    oid("1.3.6.1.2.1.1.1.0"),
                    Valeur::Octets(b"Copieur EXEMPLE".to_vec()),
                ),
                (oid("1.3.6.1.2.1.1.2.0"), Valeur::Oid(oid("1.3.6.1.4.1.99999.1"))),
                (oid("1.3.6.1.2.1.43.11.1.1.9.1.1"), Valeur::Entier(-3)),
                (oid("1.3.6.1.2.1.43.10.2.1.4.1.1"), Valeur::Compteur32(u32::MAX)),
                (oid("1.3.6.1.2.1.1.3.0"), Valeur::Ticks(128)),
                (oid("1.3.6.1.2.1.2.2.1.5.1"), Valeur::Jauge32(1_000_000_000)),
                (oid("1.3.6.1.2.1.31.1.1.1.6.1"), Valeur::Compteur64(u64::MAX)),
                (oid("1.3.6.1.2.1.4.20.1.1.1"), Valeur::Ip([192, 0, 2, 10])),
                (oid("1.3.6.1.4.1.99999.2.0"), Valeur::PasDObjet),
                (oid("1.3.6.1.4.1.99999.3.0"), Valeur::PasDInstance),
                (oid("1.3.6.1.4.1.99999.4.0"), Valeur::FinDeMib),
                (oid("2.999.300000"), Valeur::Nul),
            ],
        };
        assert_eq!(Message::decoder(&m.encoder()).unwrap(), m);
    }

    #[test]
    fn longueur_longue() {
        let long = vec![b'x'; 300];
        let m = Message {
            couples: vec![(oid("1.3.6.1.2.1.1.1.0"), Valeur::Octets(long))],
            pdu: TypePdu::Reponse,
            ..Message::requete(b"c", TypePdu::Get, 7, &[])
        };
        let o = m.encoder();
        assert_eq!(&o[..2], &[0x30, 0x82]);
        assert_eq!(Message::decoder(&o).unwrap(), m);
    }

    #[test]
    fn entiers_minimaux() {
        assert_eq!(entier_signe(0), vec![0x00]);
        assert_eq!(entier_signe(127), vec![0x7F]);
        assert_eq!(entier_signe(128), vec![0x00, 0x80]);
        assert_eq!(entier_signe(-1), vec![0xFF]);
        assert_eq!(entier_signe(-129), vec![0xFF, 0x7F]);
        assert_eq!(entier_non_signe(0), vec![0x00]);
        assert_eq!(entier_non_signe(255), vec![0x00, 0xFF]);
        for n in [i64::MIN, -65_536, -2, 0, 1, 255, 65_535, i64::MAX] {
            assert_eq!(decoder_signe(&entier_signe(n)).unwrap(), n);
        }
    }

    #[test]
    fn messages_abimes_refuses() {
        let bon = Message::requete(b"public", TypePdu::Get, 1, &[oid("1.3.6.1.2.1.1.1.0")]).encoder();
        for n in 0..bon.len() {
            assert!(Message::decoder(&bon[..n]).is_err(), "tronqué à {n} octets");
        }
        let mut trop = bon.clone();
        trop.push(0);
        assert!(Message::decoder(&trop).is_err());
        assert!(Message::decoder(b"pas du BER").is_err());
    }

    #[test]
    fn oid_texte() {
        assert_eq!(oid(".1.3.6.1").to_string(), "1.3.6.1");
        assert!("1".parse::<Oid>().is_err());
        assert!("1.3.x".parse::<Oid>().is_err());
        assert!("3.1".parse::<Oid>().is_err());
        assert!(oid("1.3.6.1.2.1.43.11").sous(&oid("1.3.6.1.2.1.43")));
        assert!(!oid("1.3.6.1.2.1.44").sous(&oid("1.3.6.1.2.1.43")));
    }

    #[test]
    fn texte_des_chaines() {
        assert_eq!(
            Valeur::Octets(b"Toner noir\0\0".to_vec()).texte().as_deref(),
            Some("Toner noir")
        );
        assert_eq!(
            Valeur::Octets(vec![0xE9, b't', 0xE9]).texte().as_deref(),
            Some("été")
        ); // Latin-1
        assert_eq!(Valeur::Octets(vec![0, 0]).texte(), None);
        assert_eq!(Valeur::Compteur32(5).nombre(), Some(5));
    }
}
