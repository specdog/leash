use std::{fs, path::PathBuf};

use sha2::{Digest, Sha256};

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn checked_in_source_and_artifact_match_the_release_contract() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("kernels/leash_kernels.cu"))
        .unwrap()
        .replace("\r\n", "\n");
    let artifact = fs::read(root.join("kernels/prebuilt/sm_87/leash_kernels.fatbin")).unwrap();

    assert_eq!(sha256(source.as_bytes()), leash_cuda::SOURCE_SHA256);
    assert_eq!(sha256(&artifact), leash_cuda::ARTIFACT_SHA256);
    assert_eq!(artifact.len(), leash_cuda::artifact().bytes);
}

#[cfg(feature = "cuda")]
#[test]
fn compiled_artifact_is_the_checked_in_fatbin() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let checked_in = fs::read(root.join("kernels/prebuilt/sm_87/leash_kernels.fatbin")).unwrap();
    assert_eq!(leash_cuda::PREBUILT_FATBIN, checked_in);
}
