//! L'exécutable réel interroge un faux copieur SNMP en UDP local.

use std::net::UdpSocket;
use std::process::Command;

use tirage_snmp::Valeur;
use tirage_snmp::client::essai::FauxAgent;

fn faux_copieur(requetes: usize) -> (String, std::thread::JoinHandle<()>) {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let adresse = s.local_addr().unwrap().to_string();
    let mut a = FauxAgent::nouveau("communaute-exemple")
        .avec("1.3.6.1.2.1.1.1.0", Valeur::Octets(b"EXEMPLE Copieur".to_vec()))
        .avec(
            "1.3.6.1.2.1.1.2.0",
            Valeur::Oid("1.3.6.1.4.1.18334.1".parse().unwrap()),
        )
        .avec("1.3.6.1.2.1.43.10.2.1.4.1.1", Valeur::Compteur32(4242))
        .avec(
            "1.3.6.1.2.1.43.11.1.1.6.1.1",
            Valeur::Octets(b"Toner noir".to_vec()),
        )
        .avec("1.3.6.1.2.1.43.11.1.1.8.1.1", Valeur::Entier(100))
        .avec("1.3.6.1.2.1.43.11.1.1.9.1.1", Valeur::Entier(10))
        .avec("1.3.6.1.4.1.18334.5.1", Valeur::Entier(1))
        .avec("1.3.6.1.4.1.18334.5.2", Valeur::Entier(2));
    let fil = std::thread::spawn(move || {
        let mut t = [0u8; 4096];
        for _ in 0..requetes {
            let (n, de) = s.recv_from(&mut t).unwrap();
            if let Some(r) = a.repondre(&t[..n]) {
                s.send_to(&r, de).unwrap();
            }
        }
    });
    (adresse, fil)
}

fn config(dossier: &std::path::Path, adresse: &str) -> std::path::PathBuf {
    let p = dossier.join("collecteur.toml");
    std::fs::write(
        &p,
        format!(
            "collecteur = \"essai\"\n[snmp]\ndelai_ms = 1000\nessais = 1\n[[copieurs]]\nnom = \"COPIEUR-EXEMPLE-1\"\nadresse = \"{adresse}\"\ncommunaute = \"communaute-exemple\"\n"
        ),
    )
    .unwrap();
    p
}

#[test]
fn releve_par_l_executable() {
    // Requêtes du relevé : ident (1) + 2 premières (2) + total (1) + 3 colonnes (2+2+2) + état (1) + erreurs (1) + alertes (1).
    let (adresse, fil) = faux_copieur(13);
    let d = tempfile::tempdir().unwrap();
    let sortie = Command::new(env!("CARGO_BIN_EXE_tirage-collecteur"))
        .args(["--releve", "--config"])
        .arg(config(d.path(), &adresse))
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&sortie.stderr);
    assert!(sortie.status.success(), "{err}");
    assert!(err.contains("profil konica-minolta"), "{err}");
    assert!(!err.contains("communaute-exemple"));
    let json: serde_json::Value = serde_json::from_slice(&sortie.stdout).unwrap();
    let r = &json["evenement"];
    assert_eq!(r["type"], "releve");
    assert_eq!(r["compteur_total"], 4242);
    assert_eq!(r["consommables"][0]["description"], "Toner noir");
    fil.join().unwrap();
}

#[test]
fn parcours_par_l_executable() {
    let (adresse, fil) = faux_copieur(3);
    let d = tempfile::tempdir().unwrap();
    let sortie = Command::new(env!("CARGO_BIN_EXE_tirage-collecteur"))
        .args([
            "--parcourir",
            "1.3.6.1.4.1.18334",
            "--copieur",
            "COPIEUR-EXEMPLE-1",
            "--config",
        ])
        .arg(config(d.path(), &adresse))
        .output()
        .unwrap();
    assert!(
        sortie.status.success(),
        "{}",
        String::from_utf8_lossy(&sortie.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&sortie.stdout),
        "1.3.6.1.4.1.18334.5.1 = INTEGER: 1\n1.3.6.1.4.1.18334.5.2 = INTEGER: 2\n"
    );
    fil.join().unwrap();
}
