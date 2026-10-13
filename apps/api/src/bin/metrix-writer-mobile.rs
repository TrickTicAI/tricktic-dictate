//! Run only behind a private HTTPS reverse proxy; upstream is deliberately loopback-only.
use metrix_writer_api::mobile;
use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::var_os("WRITER_MOBILE_DATA_DIR")
            .ok_or("Set an explicit private WRITER_MOBILE_DATA_DIR")?,
    );
    if !root.is_absolute() {
        return Err("WRITER_MOBILE_DATA_DIR must be absolute".into());
    }
    // Any internally reused settings/cleanup implementation stays in this dedicated deployment.
    std::env::set_var("WRITER_DATA_DIR", root.join("service"));
    std::env::set_var("WRITER_STATE_DIR", root.join("service/state"));
    std::env::set_var("WRITER_MODELS_DIR", root.join("service/models"));
    let args: Vec<_> = std::env::args().skip(1).collect();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(root, args))
}
async fn run(root: PathBuf, args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    match args.as_slice() {
        [cmd, output] if cmd == "issue" => { let id = mobile::issue(&root, &PathBuf::from(output)).await?; println!("Device {id} issued; credential saved to the requested private file."); }
        [cmd, id, output] if cmd == "rotate" => {
            mobile::rotate(&root, id.parse()?, &PathBuf::from(output)).await?;
            println!("Device credential replaced; previous credential is invalid and storage identity is retained.");
        }
        [cmd, id] if cmd == "revoke" => { mobile::revoke(&root, id.parse()?).await?; println!("Device revoked; retained data is unchanged."); }
        [cmd] if cmd == "serve" => {
            let port: u16 = std::env::var("WRITER_MOBILE_PORT").unwrap_or_else(|_| "18796".into()).parse()?;
            let max: u64 = std::env::var("WRITER_MAX_UPLOAD_BYTES").unwrap_or_else(|_| "1073741824".into()).parse()?;
            if max == 0 { return Err("Upload byte limit must be positive".into()); }
            let service = mobile::Mobile::open(root, max, metrix_writer_api::providers::cloud::adapters()).await.map_err(|_| "Cannot open private mobile storage; check permissions and other instances")?;
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
            eprintln!("Writer mobile upstream listening on {} (HTTPS proxy required)", listener.local_addr()?);
            axum::serve(listener, service.router()).with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; }).await?;
        }
        _ => return Err("Usage: metrix-writer-mobile issue <new-private-output-file> | revoke <device-uuid> | rotate <device-uuid> <new-private-output-file> | serve".into()),
    }
    Ok(())
}
