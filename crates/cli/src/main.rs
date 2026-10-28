fn main() -> anyhow::Result<()> {
    println!(
        "ourshell {} (protocol v{})",
        env!("CARGO_PKG_VERSION"),
        ourshell_proto::AGENT_PROTOCOL_VERSION
    );
    Ok(())
}
