// Build script for club-unison.
//
// 注: v0.7.0 で `assets/certs/` の self-signed cert 生成は廃止。
// TLS 証明書は実行時に `CertSource` (`src/network/cert.rs`) 経由で取得する
// 設計に変更したため、build.rs はソースディレクトリを一切改変しない。
// これにより `cargo publish` の verify step が通る (`Source directory was
// modified by build.rs` エラー解消)。

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile .proto files with buffa
    buffa_build::Config::new()
        .files(&["proto/creo_sync.proto"])
        .includes(&["proto/"])
        .compile()?;

    Ok(())
}
