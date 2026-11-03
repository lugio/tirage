//! Client SNMP v2c en lecture seule : GET, GETNEXT, parcours d'un sous-arbre.
//!
//! Le transport est abstrait ([`Transport`]) : UDP en vrai ([`Udp`]), un faux agent en mémoire dans les tests.

use std::fmt;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use crate::ber::{ErreurBer, Message, Oid, TypePdu, Valeur};

/// Nombre maximal d'OID par requête : certains appareils refusent les grosses requêtes (« tooBig »).
pub const OID_PAR_REQUETE: usize = 10;

/// Envoie une requête, rend la réponse brute.
pub trait Transport {
    fn echanger(&mut self, requete: &[u8]) -> io::Result<Vec<u8>>;
}

#[derive(Debug)]
pub enum ErreurSnmp {
    Io(io::Error),
    /// Pas de réponse dans le délai, après tous les essais.
    SansReponse,
    Illisible(ErreurBer),
    /// L'appareil répond par une erreur (ex. 2 = noSuchName en v1, 5 = genErr).
    Erreur {
        etat: i64,
        index: i64,
    },
    /// Réponse d'une autre requête, ou d'une forme inattendue.
    Inattendue(String),
}

impl fmt::Display for ErreurSnmp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErreurSnmp::Io(e) => write!(f, "SNMP : {e}"),
            ErreurSnmp::SansReponse => {
                f.write_str("SNMP : pas de réponse (adresse, communauté ou flux réseau ?)")
            }
            ErreurSnmp::Illisible(e) => write!(f, "SNMP : {e}"),
            ErreurSnmp::Erreur { etat, index } => write!(f, "SNMP : erreur {etat} sur l'OID n° {index}"),
            ErreurSnmp::Inattendue(m) => write!(f, "SNMP : réponse inattendue ({m})"),
        }
    }
}

impl std::error::Error for ErreurSnmp {}

impl From<ErreurBer> for ErreurSnmp {
    fn from(e: ErreurBer) -> Self {
        ErreurSnmp::Illisible(e)
    }
}

/// Transport UDP : délai par essai et nombre d'essais.
pub struct Udp {
    socket: UdpSocket,
    cible: SocketAddr,
    essais: u32,
}

impl Udp {
    /// `adresse` : « hôte » ou « hôte:port » (161 par défaut).
    pub fn ouvrir(adresse: &str, delai: Duration, essais: u32) -> io::Result<Self> {
        let avec_port = if adresse.parse::<SocketAddr>().is_ok() || adresse.matches(':').count() == 1 {
            adresse.to_owned()
        } else if let Ok(ip6) = adresse.parse::<std::net::Ipv6Addr>() {
            format!("[{ip6}]:161")
        } else {
            format!("{adresse}:161")
        };
        let cible = avec_port.to_socket_addrs()?.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("adresse introuvable : {adresse}"),
            )
        })?;
        let locale: SocketAddr = if cible.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }
            .parse()
            .expect("adresse locale valide");
        let socket = UdpSocket::bind(locale)?;
        socket.connect(cible)?;
        socket.set_read_timeout(Some(delai))?;
        Ok(Udp {
            socket,
            cible,
            essais: essais.max(1),
        })
    }

    pub fn cible(&self) -> SocketAddr {
        self.cible
    }
}

impl Transport for Udp {
    fn echanger(&mut self, requete: &[u8]) -> io::Result<Vec<u8>> {
        let mut tampon = vec![0u8; 65_535];
        for _ in 0..self.essais {
            self.socket.send(requete)?;
            match self.socket.recv(&mut tampon) {
                Ok(n) => return Ok(tampon[..n].to_vec()),
                // Délai dépassé, ou « port injoignable » renvoyé par l'hôte (Windows : réinitialisation) : on réessaie.
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "pas de réponse"))
    }
}

pub struct Client<T: Transport> {
    transport: T,
    communaute: Vec<u8>,
    prochain_id: i32,
}

impl<T: Transport> Client<T> {
    pub fn nouveau(transport: T, communaute: &[u8]) -> Self {
        Client {
            transport,
            communaute: communaute.to_vec(),
            prochain_id: 1,
        }
    }

    fn requete(&mut self, pdu: TypePdu, oids: &[Oid]) -> Result<Vec<(Oid, Valeur)>, ErreurSnmp> {
        let id = self.prochain_id;
        self.prochain_id = self.prochain_id.checked_add(1).filter(|&n| n > 0).unwrap_or(1);
        let octets = Message::requete(&self.communaute, pdu, id, oids).encoder();
        let reponse = match self.transport.echanger(&octets) {
            Ok(r) => r,
            Err(e) if e.kind() == io::ErrorKind::TimedOut => return Err(ErreurSnmp::SansReponse),
            Err(e) => return Err(ErreurSnmp::Io(e)),
        };
        let m = Message::decoder(&reponse)?;
        if m.pdu != TypePdu::Reponse {
            return Err(ErreurSnmp::Inattendue("PDU qui n'est pas une réponse".into()));
        }
        if m.id_requete != id {
            return Err(ErreurSnmp::Inattendue(format!(
                "identifiant {} au lieu de {id}",
                m.id_requete
            )));
        }
        if m.etat_erreur != 0 {
            return Err(ErreurSnmp::Erreur {
                etat: m.etat_erreur,
                index: m.index_erreur,
            });
        }
        if m.couples.len() != oids.len() {
            return Err(ErreurSnmp::Inattendue(format!(
                "{} valeur(s) pour {} OID",
                m.couples.len(),
                oids.len()
            )));
        }
        Ok(m.couples)
    }

    /// Lit des OID précis. Un objet absent revient en exception (`PasDObjet`, `PasDInstance`), pas en erreur.
    pub fn get(&mut self, oids: &[Oid]) -> Result<Vec<(Oid, Valeur)>, ErreurSnmp> {
        let mut tout = Vec::with_capacity(oids.len());
        for paquet in oids.chunks(OID_PAR_REQUETE) {
            tout.extend(self.requete(TypePdu::Get, paquet)?);
        }
        Ok(tout)
    }

    /// Une seule valeur, `None` si l'objet est absent.
    pub fn valeur(&mut self, oid: &Oid) -> Result<Option<Valeur>, ErreurSnmp> {
        let v = self.get(std::slice::from_ref(oid))?.pop().map(|(_, v)| v);
        Ok(v.filter(|v| !v.est_exception()))
    }

    /// Parcourt le sous-arbre `base` par GETNEXT, au plus `max` valeurs.
    /// S'arrête à la sortie du sous-arbre, à la fin de la MIB, ou si l'appareil ne progresse plus.
    pub fn parcourir(&mut self, base: &Oid, max: usize) -> Result<Vec<(Oid, Valeur)>, ErreurSnmp> {
        let mut resultats = Vec::new();
        let mut courant = base.clone();
        while resultats.len() < max {
            let (oid, v) = match self.requete(TypePdu::GetNext, std::slice::from_ref(&courant)) {
                Ok(mut c) => c.pop().expect("une valeur vérifiée"),
                // Agents v1 déguisés : noSuchName (2) en fin de MIB.
                Err(ErreurSnmp::Erreur { etat: 2, .. }) => break,
                Err(e) => return Err(e),
            };
            if v == Valeur::FinDeMib || !oid.sous(base) {
                break;
            }
            if oid <= courant {
                return Err(ErreurSnmp::Inattendue(format!(
                    "l'appareil ne progresse plus après {courant}"
                )));
            }
            courant = oid.clone();
            resultats.push((oid, v));
        }
        Ok(resultats)
    }

    /// Première valeur d'un sous-arbre (ex. première ligne d'une colonne de table), si elle existe.
    pub fn premiere(&mut self, base: &Oid) -> Result<Option<(Oid, Valeur)>, ErreurSnmp> {
        Ok(self.parcourir(base, 1)?.pop())
    }
}

/// Faux agent SNMP pour les tests : une MIB en mémoire, répond comme un appareil v2c.
pub mod essai {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default, Clone)]
    pub struct FauxAgent {
        pub communaute: Vec<u8>,
        pub mib: BTreeMap<Oid, Valeur>,
        /// Nombre de requêtes reçues (pour vérifier le découpage).
        pub requetes: usize,
    }

    impl FauxAgent {
        pub fn nouveau(communaute: &str) -> Self {
            FauxAgent {
                communaute: communaute.as_bytes().to_vec(),
                ..Default::default()
            }
        }

        pub fn avec(mut self, oid: &str, v: Valeur) -> Self {
            self.mib.insert(oid.parse().expect("OID d'essai valide"), v);
            self
        }

        /// Réponse à une requête ; `None` = mauvaise communauté (un vrai agent se tait).
        pub fn repondre(&mut self, requete: &[u8]) -> Option<Vec<u8>> {
            self.requetes += 1;
            let m = Message::decoder(requete).ok()?;
            if m.communaute != self.communaute {
                return None;
            }
            let couples =
                m.couples
                    .iter()
                    .map(|(oid, _)| match m.pdu {
                        TypePdu::GetNext => match self
                            .mib
                            .range((std::ops::Bound::Excluded(oid.clone()), std::ops::Bound::Unbounded))
                            .next()
                        {
                            Some((o, v)) => (o.clone(), v.clone()),
                            None => (oid.clone(), Valeur::FinDeMib),
                        },
                        _ => {
                            let v =
                                self.mib.get(oid).cloned().unwrap_or_else(|| {
                                    if self.mib.keys().any(|k| {
                                        k.sous(&Oid(oid.0[..oid.0.len().saturating_sub(1)].to_vec()))
                                    }) {
                                        Valeur::PasDInstance
                                    } else {
                                        Valeur::PasDObjet
                                    }
                                });
                            (oid.clone(), v)
                        }
                    })
                    .collect();
            Some(
                Message {
                    pdu: TypePdu::Reponse,
                    couples,
                    ..m
                }
                .encoder(),
            )
        }
    }

    impl Transport for FauxAgent {
        fn echanger(&mut self, requete: &[u8]) -> io::Result<Vec<u8>> {
            self.repondre(requete)
                .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "pas de réponse"))
        }
    }

    impl Transport for &mut FauxAgent {
        fn echanger(&mut self, requete: &[u8]) -> io::Result<Vec<u8>> {
            (**self).echanger(requete)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::essai::FauxAgent;
    use super::*;

    fn oid(s: &str) -> Oid {
        s.parse().unwrap()
    }

    fn agent() -> FauxAgent {
        FauxAgent::nouveau("exemple")
            .avec("1.3.6.1.2.1.1.1.0", Valeur::Octets(b"Copieur EXEMPLE".to_vec()))
            .avec(
                "1.3.6.1.2.1.43.11.1.1.6.1.1",
                Valeur::Octets(b"Toner noir".to_vec()),
            )
            .avec(
                "1.3.6.1.2.1.43.11.1.1.6.1.2",
                Valeur::Octets(b"Toner cyan".to_vec()),
            )
            .avec("1.3.6.1.2.1.43.11.1.1.8.1.1", Valeur::Entier(100))
            .avec("1.3.6.1.2.1.44.1.0", Valeur::Entier(1))
    }

    #[test]
    fn get_et_objet_absent() {
        let mut a = agent();
        let mut c = Client::nouveau(&mut a, b"exemple");
        assert_eq!(
            c.valeur(&oid("1.3.6.1.2.1.1.1.0")).unwrap(),
            Some(Valeur::Octets(b"Copieur EXEMPLE".to_vec()))
        );
        assert_eq!(c.valeur(&oid("1.3.6.1.2.1.1.5.0")).unwrap(), None);
    }

    #[test]
    fn parcours_borne_au_sous_arbre() {
        let mut a = agent();
        let mut c = Client::nouveau(&mut a, b"exemple");
        let r = c.parcourir(&oid("1.3.6.1.2.1.43.11.1.1.6"), 100).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].1.texte().as_deref(), Some("Toner cyan"));
        assert_eq!(
            c.parcourir(&oid("1.3.6.1.2.1.43"), 2).unwrap().len(),
            2,
            "borne max respectée"
        );
        assert!(
            c.parcourir(&oid("1.3.6.1.2.1.45"), 10).unwrap().is_empty(),
            "fin de MIB"
        );
    }

    #[test]
    fn decoupage_des_grosses_requetes() {
        let mut a = agent();
        let oids: Vec<Oid> = (0..25).map(|i| oid(&format!("1.3.6.1.4.1.99999.{i}"))).collect();
        let r = Client::nouveau(&mut a, b"exemple").get(&oids).unwrap();
        assert_eq!(r.len(), 25);
        assert_eq!(a.requetes, 3);
    }

    #[test]
    fn mauvaise_communaute_sans_reponse() {
        let mut a = agent();
        let mut c = Client::nouveau(&mut a, b"autre");
        assert!(matches!(
            c.valeur(&oid("1.3.6.1.2.1.1.1.0")),
            Err(ErreurSnmp::SansReponse)
        ));
    }

    struct Bavard;
    impl Transport for Bavard {
        fn echanger(&mut self, requete: &[u8]) -> io::Result<Vec<u8>> {
            let m = Message::decoder(requete).unwrap();
            Ok(Message {
                pdu: TypePdu::Reponse,
                id_requete: m.id_requete + 1000,
                ..m
            }
            .encoder())
        }
    }

    #[test]
    fn reponse_d_une_autre_requete_refusee() {
        let mut c = Client::nouveau(Bavard, b"x");
        assert!(matches!(
            c.get(&[oid("1.3.6.1.2.1.1.1.0")]),
            Err(ErreurSnmp::Inattendue(_))
        ));
    }

    struct Immobile;
    impl Transport for Immobile {
        fn echanger(&mut self, requete: &[u8]) -> io::Result<Vec<u8>> {
            let m = Message::decoder(requete).unwrap();
            let base = m.couples[0].0.clone();
            Ok(Message {
                pdu: TypePdu::Reponse,
                couples: vec![(base, Valeur::Entier(0))],
                ..m
            }
            .encoder())
        }
    }

    #[test]
    fn appareil_qui_boucle_arrete_le_parcours() {
        let mut c = Client::nouveau(Immobile, b"x");
        assert!(matches!(
            c.parcourir(&oid("1.3.6.1"), 1000),
            Err(ErreurSnmp::Inattendue(_))
        ));
    }

    #[test]
    fn vrai_udp_en_local() {
        let serveur = UdpSocket::bind("127.0.0.1:0").unwrap();
        let adresse = serveur.local_addr().unwrap().to_string();
        let mut a = agent();
        let fil = std::thread::spawn(move || {
            let mut t = [0u8; 2048];
            for _ in 0..2 {
                let (n, de) = serveur.recv_from(&mut t).unwrap();
                if let Some(r) = a.repondre(&t[..n]) {
                    serveur.send_to(&r, de).unwrap();
                }
            }
        });
        let udp = Udp::ouvrir(&adresse, Duration::from_millis(500), 1).unwrap();
        let mut c = Client::nouveau(udp, b"exemple");
        assert!(c.valeur(&oid("1.3.6.1.2.1.1.1.0")).unwrap().is_some());
        assert_eq!(
            c.premiere(&oid("1.3.6.1.2.1.44")).unwrap().unwrap().1,
            Valeur::Entier(1)
        );
        fil.join().unwrap();
    }

    #[test]
    fn udp_sans_reponse() {
        let muet = UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp = Udp::ouvrir(
            &muet.local_addr().unwrap().to_string(),
            Duration::from_millis(50),
            2,
        )
        .unwrap();
        let mut c = Client::nouveau(udp, b"x");
        assert!(matches!(
            c.valeur(&oid("1.3.6.1.2.1.1.1.0")),
            Err(ErreurSnmp::SansReponse)
        ));
    }
}
