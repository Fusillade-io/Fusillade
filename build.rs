fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/cluster.proto");
    tonic_build::compile_protos("proto/cluster.proto")?;
    Ok(())
}
