//! T066 — Resolução do diretório de binários/recursos empacotados pelo
//! instalador, por layout de pacote.
//!
//! O layout do Windows (T067) é `<dir do exe>/bin/` — o NSIS/MSI instala
//! tudo lado a lado. O Linux não: o `tauri-bundler` põe o executável em
//! `/usr/bin/<binário>` e os `bundle.resources` em
//! `/usr/lib/<productName>/` (mesma regra que o `resource_dir()` do
//! `tauri-utils` usa: `<dir do exe>/../lib/<productName>`, com
//! `$APPDIR/usr/lib/<productName>` quando roda via AppImage e
//! `/usr/lib/<productName>` como último recurso).
//!
//! Sem isso o jar do fork embutido (FR-024) não é encontrado no pacote
//! instalado — `bundled_file()` procurava só `<exe>/bin/`, que no `.deb`
//! seria `/usr/bin/bin/`.
//!
//! Escritos ANTES da implementação (Princípio III). Lógica pura de
//! caminho: roda igual nas duas plataformas (Princípio IV), os candidatos
//! que não existem no SO corrente simplesmente não casam em disco.

use std::path::{Path, PathBuf};

use camlink_lib::paths::{bin_dir_candidates, PRODUCT_NAME};

fn candidates(exe_dir: &str, appdir: Option<&str>) -> Vec<PathBuf> {
    bin_dir_candidates(Path::new(exe_dir), appdir.map(Path::new))
}

#[test]
fn windows_and_dev_layout_comes_first() {
    // Instalação NSIS/MSI (T067) e `cargo tauri dev`: binários ao lado do
    // executável. Continua sendo o primeiro candidato — a mudança do T066
    // não pode alterar o comportamento já validado no Windows.
    let found = candidates("/opt/CamLink", None);
    assert_eq!(found.first().unwrap(), Path::new("/opt/CamLink/bin"));
}

#[test]
fn deb_layout_resolves_resources_next_to_lib() {
    // `.deb`: /usr/bin/camlink + /usr/lib/CamLink/bin/<recursos>.
    let found = candidates("/usr/bin", None);
    let expected = PathBuf::from(format!("/usr/lib/{PRODUCT_NAME}/bin"));
    assert!(
        found.contains(&expected),
        "layout .deb ausente nos candidatos: {found:?}"
    );
}

#[test]
fn deb_layout_is_relative_to_the_executable_not_hardcoded_usr() {
    // Prefixo diferente de `/usr` (instalação em /opt, `--prefix` custom):
    // o candidato tem que sair do diretório do executável, não de um
    // `/usr/lib` fixo.
    let found = candidates("/opt/camlink/bin", None);
    let expected = PathBuf::from(format!("/opt/camlink/lib/{PRODUCT_NAME}/bin"));
    assert!(
        found.contains(&expected),
        "candidato relativo ao exe ausente: {found:?}"
    );
}

#[test]
fn appimage_layout_uses_appdir_when_set() {
    // AppImage: o exe roda de $APPDIR/usr/bin e os resources ficam em
    // $APPDIR/usr/lib/<productName>.
    let found = candidates("/tmp/.mount_CamLix/usr/bin", Some("/tmp/.mount_CamLix"));
    let expected = PathBuf::from(format!("/tmp/.mount_CamLix/usr/lib/{PRODUCT_NAME}/bin"));
    assert!(
        found.contains(&expected),
        "layout AppImage ausente nos candidatos: {found:?}"
    );
}

#[test]
fn appdir_absent_yields_no_appimage_candidate() {
    let found = candidates("/usr/bin", None);
    assert!(
        !found.iter().any(|p| p.starts_with("/tmp/.mount")),
        "candidato de AppImage não deveria existir sem APPDIR: {found:?}"
    );
}

#[test]
fn system_wide_fallback_is_last_resort() {
    // Mesmo fallback final do `resource_dir()` do tauri-utils, para quando
    // o executável é chamado por um caminho que não permite derivar o
    // prefixo (symlink em ~/.local/bin, wrapper de sessão).
    let found = candidates("/home/user/.local/bin", None);
    let expected = PathBuf::from(format!("/usr/lib/{PRODUCT_NAME}/bin"));
    assert_eq!(
        found.last().unwrap(),
        &expected,
        "fallback /usr/lib deveria ser o último candidato: {found:?}"
    );
}

#[test]
fn candidates_are_unique_and_ordered_from_most_specific() {
    // `/usr/bin` gera o mesmo `/usr/lib/<produto>/bin` pelo caminho
    // relativo e pelo fallback absoluto — não pode duplicar (o chamador
    // testa existência em disco na ordem, duplicata é trabalho repetido).
    let found = candidates("/usr/bin", None);
    let mut deduped = found.clone();
    deduped.dedup();
    assert_eq!(found, deduped, "candidatos duplicados: {found:?}");
    assert_eq!(found.first().unwrap(), Path::new("/usr/bin/bin"));
}

#[test]
fn exe_at_filesystem_root_does_not_panic() {
    // Caminho degenerado (sem pai): não pode entrar em pânico — este código
    // roda na inicialização do app.
    let found = candidates("/", None);
    assert!(!found.is_empty());
}
