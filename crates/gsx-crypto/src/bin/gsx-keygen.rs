use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Generate ML-DSA-65 keypairs for the gsx-dag network")]
struct Cli {
    #[arg(long, default_value = "mldsa")]
    algo: String,
    #[arg(long)]
    sk: PathBuf,
    #[arg(long)]
    pk: PathBuf,
}

fn main() {
    let cli = Cli::parse();
    if cli.algo != "mldsa" {
        eprintln!("error: only --algo mldsa is supported");
        std::process::exit(1);
    }
    let (pk, sk) = gsx_crypto::mldsa::keypair();
    std::fs::write(&cli.pk, pk.as_bytes()).expect("write public key");
    std::fs::write(&cli.sk, sk.as_bytes()).expect("write secret key");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli.sk, std::fs::Permissions::from_mode(0o600))
            .expect("chmod sk");
    }
    let hash = blake3::hash(pk.as_bytes());
    let addr = &hash.as_bytes()[..20];
    eprintln!(
        "pk:      {} bytes → {}",
        pk.as_bytes().len(),
        cli.pk.display()
    );
    eprintln!(
        "sk:      {} bytes → {}",
        sk.as_bytes().len(),
        cli.sk.display()
    );
    eprintln!("address: 0x{}", hex::encode(addr));
}
