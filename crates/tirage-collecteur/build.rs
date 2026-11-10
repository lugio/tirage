//! Intègre à l'exécutable tous les profils de `profils/*.toml` : ajouter une marque = déposer un fichier.

use std::path::Path;

fn main() {
    let dossier = Path::new(env!("CARGO_MANIFEST_DIR")).join("profils");
    println!("cargo:rerun-if-changed={}", dossier.display());
    let mut fichiers: Vec<_> = std::fs::read_dir(&dossier)
        .expect("dossier profils/")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .collect();
    fichiers.sort();
    let mut code = String::from("&[\n");
    for p in &fichiers {
        let nom = p
            .file_name()
            .and_then(|n| n.to_str())
            .expect("nom de fichier UTF-8");
        let chemin = p.to_str().expect("chemin UTF-8");
        code.push_str(&format!("    ({nom:?}, include_str!({chemin:?})),\n"));
    }
    code.push(']');
    let sortie = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("profils_integres.rs");
    std::fs::write(sortie, code).expect("écriture de la liste des profils");
}
