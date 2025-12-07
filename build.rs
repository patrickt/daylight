use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate FlatBuffers Rust code using flatc
    let status = Command::new("flatc")
        .args(&["--rust", "-o", "lib/generated/", "daylight.fbs"])
        .status();
    
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rerun-if-changed=daylight.fbs");
            Ok(())
        }
        Ok(s) => {
            eprintln!("flatc failed with status: {}", s);
            Err("flatc compilation failed".into())
        }
        Err(e) => {
            eprintln!("Error running flatc: {}. Please install flatbuffers.", e);
            eprintln!("On macOS: brew install flatbuffers");
            eprintln!("On Ubuntu: apt-get install flatbuffers-compiler");
            Err(e.into())
        }
    }
}
