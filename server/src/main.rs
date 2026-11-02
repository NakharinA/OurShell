#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => ourshell_server::run().await,
        Some("healthcheck") => ourshell_server::healthcheck().await,
        Some("set-admin-password") => ourshell_server::set_admin_password(),
        Some("restore") => ourshell_server::backup::restore::run_cli(&args[1..]).await,
        Some(other) => {
            eprintln!(
                "usage: ourshell-server [serve | healthcheck | set-admin-password | restore <file.osbk | s3:KEY | s3:latest> [--dry-run]]
                 unknown command `{other}`"
            );
            std::process::exit(2);
        }
    }
}
