//! Onde procurar os binários/recursos que o instalador embute (T066/T067).
//!
//! Cada bundler usa um layout diferente e o app precisa achar o jar do fork
//! (FR-024) — e, no Windows, também `adb`/`ffmpeg` — sem depender do PATH:
//!
//! | pacote          | executável                  | recursos                              |
//! |-----------------|-----------------------------|---------------------------------------|
//! | NSIS/MSI (T067) | `<instal>\CamLink.exe`      | `<instal>\bin\`                       |
//! | `.deb` (T066)   | `/usr/bin/camlink`          | `/usr/lib/CamLink/bin/`               |
//! | AppImage (T066) | `$APPDIR/usr/bin/camlink`   | `$APPDIR/usr/lib/CamLink/bin/`        |
//! | `cargo tauri dev` | `target/debug/camlink`    | nenhum (cai no PATH — ver `lib.rs`)   |
//!
//! Os candidatos do Linux seguem a mesma regra do `resource_dir()` do
//! `tauri-utils` (`<dir do exe>/../lib/<productName>`, `$APPDIR/usr/lib/…`,
//! `/usr/lib/…` como último recurso), só que sem exigir um `AppHandle` —
//! `resolve_external_paths()` é chamado de contextos que não têm um
//! (cleanups na inicialização, tarefas de sessão).
//!
//! Lógica pura de caminho, sem `#[cfg]`: roda igual nas duas plataformas
//! (Princípio IV) e os candidatos irrelevantes para o SO corrente
//! simplesmente não existem em disco.

use std::path::{Path, PathBuf};

/// `productName` do `tauri.conf.json` — é o nome que o bundler usa no
/// diretório de recursos (`/usr/lib/<productName>`) e o mesmo que o
/// `resource_dir()` do Tauri deriva do `PackageInfo`.
pub const PRODUCT_NAME: &str = "CamLink";

/// Subdiretório dentro do diretório de recursos onde o instalador coloca os
/// binários embutidos (espelha o mapeamento `bundle.resources`).
const BIN_SUBDIR: &str = "bin";

/// Candidatos ordenados (mais específico primeiro) a diretório de binários
/// embutidos, derivados do diretório do executável e do `$APPDIR` (definido
/// só quando o app roda a partir de um AppImage). Sem duplicatas: o
/// chamador testa existência em disco na ordem.
pub fn bin_dir_candidates(exe_dir: &Path, appdir: Option<&Path>) -> Vec<PathBuf> {
    let system_lib = Path::new("/usr/lib").join(PRODUCT_NAME).join(BIN_SUBDIR);
    let mut candidates = vec![exe_dir.join(BIN_SUBDIR)];

    // `.deb` e AppImage: <prefixo>/bin/<exe> → <prefixo>/lib/<produto>/bin.
    if let Some(prefix) = exe_dir.parent() {
        candidates.push(prefix.join("lib").join(PRODUCT_NAME).join(BIN_SUBDIR));
    }
    if let Some(appdir) = appdir {
        candidates.push(appdir.join("usr/lib").join(PRODUCT_NAME).join(BIN_SUBDIR));
    }
    candidates.push(system_lib);

    candidates.dedup();
    candidates
}

/// Primeiro candidato que existe em disco, a partir do executável corrente.
/// `None` em dev (nenhum layout de instalação bate), o que faz o chamador
/// cair no fallback de PATH sem mudar o comportamento de `cargo tauri dev`.
pub fn bundled_bin_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let appdir = std::env::var_os("APPDIR").map(PathBuf::from);
    bin_dir_candidates(exe_dir, appdir.as_deref())
        .into_iter()
        .find(|dir| dir.is_dir())
}
