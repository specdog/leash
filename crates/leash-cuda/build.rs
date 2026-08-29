use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const ARTIFACT_NAME: &str = "leash_kernels.fatbin";

fn main() {
    println!("cargo:rerun-if-changed=kernels/leash_kernels.cu");
    println!("cargo:rerun-if-changed=kernels/prebuilt/sm_87/{ARTIFACT_NAME}");
    println!("cargo:rerun-if-changed=kernels/prebuilt/sm_87/manifest.json");
    println!("cargo:rerun-if-env-changed=LEASH_CUDA_REBUILD");
    println!("cargo:rerun-if-env-changed=NVCC");

    if env::var_os("CARGO_FEATURE_CUDA").is_none() {
        return;
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = manifest_dir.join("kernels/leash_kernels.cu");
    let prebuilt = manifest_dir
        .join("kernels/prebuilt/sm_87")
        .join(ARTIFACT_NAME);
    let output = out_dir.join(ARTIFACT_NAME);

    if env::var_os("LEASH_CUDA_REBUILD").as_deref() == Some(OsStr::new("1")) {
        rebuild(&source, &output);
    } else {
        fs::copy(&prebuilt, &output).unwrap_or_else(|error| {
            panic!(
                "copy prebuilt CUDA artifact {} to {}: {error}",
                prebuilt.display(),
                output.display()
            )
        });
    }
}

fn rebuild(source: &Path, output: &Path) {
    let nvcc = env::var_os("NVCC").unwrap_or_else(|| "nvcc".into());
    let status = Command::new(&nvcc)
        .args([
            "--fatbin",
            "-O3",
            "-std=c++14",
            "-gencode=arch=compute_87,code=sm_87",
            "-gencode=arch=compute_87,code=compute_87",
        ])
        .arg(source)
        .arg("-o")
        .arg(output)
        .status()
        .unwrap_or_else(|error| panic!("start {:?}: {error}", nvcc));
    assert!(status.success(), "nvcc failed with {status}");
}
