fn main() -> Result<(), Box<dyn std::error::Error>> {
    capnpc::CompilerCommand::new()
        .file("daylight.capnp")
        .run()?;
    Ok(())
}
