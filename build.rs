#![allow(unused)]

use std::iter::FromIterator;
use std::collections::HashSet;
use std::convert::AsRef;
use std::path::{PathBuf, Path};
use std::string::ToString;
use tar::Archive;
use flate2::read::GzDecoder;
use std::process::{Command, Stdio};
use std::io::Write;

///////////////////////////////////////////////////////////////////////////////
// UTILS - ENVIROMENT
///////////////////////////////////////////////////////////////////////////////

fn out_dir() -> PathBuf {
    PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR env var"))
}

fn is_release_mode() -> bool {
    has_env_var_with_value("PROFILE", "release")
}

fn is_debug_mode() -> bool {
    has_env_var_with_value("PROFILE", "debug")
}

fn opt_level_eq(x: u8) -> bool {
    has_env_var_with_value("OPT_LEVEL", &format!("{}", x))
}

fn has_env_var_with_value(s: &str, v: &str) -> bool {
    std::env::var(s)
        .map(|x| x.to_lowercase())
        .map(|x| x == v.to_lowercase())
        .unwrap_or(false)
}

///////////////////////////////////////////////////////////////////////////////
// UTILS - BUILD
///////////////////////////////////////////////////////////////////////////////

pub fn extract_tar_file<P: AsRef<Path>, Q: AsRef<Path>>(tar_file: P, dest: Q) -> Result<(), String> {
    let source = std::fs::read(tar_file).expect("read tar file");
    let tar = GzDecoder::new(&source[..]);
    let mut archive = Archive::new(tar);
    // UNPACK ARCHIVE
    let tmp_source_dir: Option<PathBuf> = {
        archive
            .unpack(&dest)
            .map_err(|x| format!("[{:?}] failed to unpack tar file: {:?}", dest.as_ref(), x))?;
        let xs = std::fs::read_dir(&dest)
            .expect(&format!("unable to read dir {:?}", dest.as_ref()))
            .filter_map(Result::ok)
            .filter(|file| file.file_type().map(|x| x.is_dir()).unwrap_or(false))
            .collect::<Vec<std::fs::DirEntry>>();
        match &xs[..] {
            [x] => Some(x.path()),
            _ => None,
        }
    };
    Ok(())
}

pub fn lookup_newest(paths: Vec<PathBuf>) -> Option<PathBuf> {
    use std::time::{SystemTime, Duration};
    let mut newest: Option<(PathBuf, Duration)> = None;
    paths
        .clone()
        .into_iter()
        .filter_map(|x: PathBuf| {
            let timestamp = x
                .metadata()
                .ok()
                .and_then(|y| y.created().ok())
                .and_then(|x| x.duration_since(SystemTime::UNIX_EPOCH).ok());
            match timestamp {
                Some(y) => Some((x, y)),
                _ => None
            }
        })
        .for_each(|(x_path, x_created)| match &newest {
            None => {
                newest = Some((x_path, x_created));
            }
            Some((_, y_created)) => {
                if &x_created > y_created {
                    newest = Some((x_path, x_created));
                }
            }
        });
    // DONE
    newest.map(|(x, _)| x)
}

pub fn files_with_prefix(dir: &PathBuf, pattern: &str) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect(&format!("get dir contents: {:?}", dir))
        .filter_map(Result::ok)
        .filter_map(|x| {
            let file_name = x
                .file_name()
                .to_str()?
                .to_owned();
            if file_name.starts_with(pattern) {
                Some(x.path())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
}

fn run_make(source_path: &PathBuf, makefile: &str) {
    let result = std::process::Command::new("make")
        .arg("-C")
        .arg(source_path)
        .arg("-f")
        .arg(makefile)
        .output()
        .expect(&format!("make -C {:?} failed", source_path));
    assert!(result.status.success());
}

fn cpy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) {
    std::fs::copy(&from, &to)
        .expect(&format!(
            "unable to cpy from {:?} to {:?}",
            from.as_ref(),
            to.as_ref(),
        ));
}

///////////////////////////////////////////////////////////////////////////////
// PATHS
///////////////////////////////////////////////////////////////////////////////

pub const STATIC_LIBS: &[(&str, &str)] = &[
    (
        "avcodec",
        "libavcodec/libavcodec.a",
    ),
    (
        "avdevice",
        "libavdevice/libavdevice.a",
    ),
    (
        "avfilter",
        "libavfilter/libavfilter.a",
    ),
    (
        "avformat",
        "libavformat/libavformat.a",
    ),
    (
        "avutil",
        "libavutil/libavutil.a",
    ),
    (
        "swresample",
        "libswresample/libswresample.a",
    ),
    (
        "swscale",
        "libswscale/libswscale.a",
    ),
];

pub const SEARCH_PATHS: &[&str] = &[
    "libavcodec",
    "libavdevice",
    "libavfilter",
    "libavformat",
    "libavresample",
    "libavutil",
    "libpostproc",
    "libswresample",
    "libswscale",
];

///////////////////////////////////////////////////////////////////////////////
// CODEGEN
///////////////////////////////////////////////////////////////////////////////

// See https://github.com/rust-lang/rust-bindgen/issues/687#issuecomment-450750547
#[derive(Debug, Clone)]
struct IgnoreMacros(HashSet<String>);

impl bindgen::callbacks::ParseCallbacks for IgnoreMacros {
    fn will_parse_macro(&self, name: &str) -> bindgen::callbacks::MacroParsingBehavior {
        if self.0.contains(name) {
            bindgen::callbacks::MacroParsingBehavior::Ignore
        } else {
            bindgen::callbacks::MacroParsingBehavior::Default
        }
    }
}


///////////////////////////////////////////////////////////////////////////////
// BUILD PIPELINE
///////////////////////////////////////////////////////////////////////////////

fn build() {
    let out_path = out_dir();
    let source_path = out_path.join("FFmpeg-FFmpeg-2722fc2");
    // SPEED UP DEV - UNLESS IN RELASE MODE
    let already_built = {
        STATIC_LIBS
            .iter()
            .map(|(_, x)| source_path.join(x))
            .all(|x| x.exists())
    };
    let mut skip_build = already_built && !is_release_mode();
    if has_env_var_with_value("FFDEV1", "1") {
        skip_build = false;
    }
    // TODO: Build with vcpkg
    #[cfg(target_os = "windows")]
    let skip_build = true;

    // EXTRACT
    if !source_path.exists() || !skip_build {
        #[cfg(not(target_os = "windows"))]
        {
            let result = std::process::Command::new("tar")
                .arg("-xJf")
                .arg("archive/FFmpeg-FFmpeg-2722fc2.tar.xz")
                .arg("-C")
                .arg(out_path.to_str().expect("PathBuf to str"))
                .output()
                .expect("tar decompression of ffmpeg source repo using xz (to fit the 10M crates limit)");
            assert!(result.status.success());
        }
        #[cfg(target_os = "windows")]
        {
            let mut result = std::process::Command::new("7z")
                .arg("x")
                .arg("-so")
                .arg("archive/FFmpeg-FFmpeg-2722fc2.tar.xz")
                .stdout(std::process::Stdio::piped())
                .output()
                .expect("tar decompression of ffmpeg source repo using xz (to fit the 10M crates limit)");
            assert!(result.status.success());
            let mut child = std::process::Command::new("7z")
                .arg("x")
                .arg("-si")
                .arg("-ttar")
                .arg(format!("-o{}", out_path.to_str().expect("PathBuf to str")))
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            eprintln!("{:?}", out_path.to_str());
            {
                let stdin = child.stdin.as_mut().unwrap();
                stdin.write_all(&result.stdout).expect("Failed to write to stdin!");
            }
            let result = child.wait_with_output().unwrap();
            assert!(result.status.success());

            // copy needed files over 
            std::fs::copy(
                "D:\\Robert\\repos\\vcpkg\\installed\\x64-windows-static\\include\\libavutil\\avconfig.h",
                source_path.join("libavutil\\avconfig.h")
            ).unwrap();
            std::fs::copy(
                "D:\\Robert\\repos\\vcpkg\\installed\\x64-windows-static\\extra\\config.h",
                source_path.join("config.h")
            ).unwrap();
        }
        assert!(source_path.exists());  
    }
    // BUILD CODE PHASE
    if skip_build == false {
        // CONFIGURE
        {
            let mut configure_flags = vec![
                "--disable-programs",
                "--disable-doc",
                "--disable-autodetect",
            ];
            // TRY TO SPEED THIS UP FOR DEV BUILDS
            if is_debug_mode() && opt_level_eq(0) {
                configure_flags.push("--disable-optimizations");
                configure_flags.push("--disable-debug");
                configure_flags.push("--disable-stripping");
            }
            let eval_configure = |flags: Vec<&str>| {
                let flags = flags.join(" ");
                std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&format!(
                        "cd {path} && ./configure {flags}",
                        path=source_path.to_str().expect("PathBuf to str"),
                        flags=flags,
                    ))
                    .output()
                    .expect(&format!("ffmpeg configure script"))
            };
            let result = eval_configure(configure_flags.clone());
            if !result.status.success() {
                let stderr = String::from_utf8(result.stderr).expect("invalid str");
                let stdout = String::from_utf8(result.stdout).expect("invalid str");
                let nasm_yasm_issue = stderr
                    .lines()
                    .chain(stdout.lines())
                    .any(|x| x.contains("nasm/yasm not found or too old"));
                // MAYBE RETRY (USE CRIPPLED BUILD)
                if nasm_yasm_issue {
                    configure_flags.push("--disable-x86asm");
                    let result = eval_configure(configure_flags);
                    if !result.status.success() {
                        let stderr = String::from_utf8(result.stderr).expect("invalid str");
                        let stdout = String::from_utf8(result.stdout).expect("invalid str");
                        panic!("configure failed:\n{}", vec![stderr, stdout].join("\n"));
                    }
                } else {
                    panic!("configure failed:\n{}", vec![stderr, stdout].join("\n"));
                }
            }
        }
        // BUILD
        {
            let mut cpu_number = num_cpus::get();
            let result = std::process::Command::new("make")
                .arg("-C")
                .arg(&source_path)
                .arg("-f")
                .arg("Makefile")
                .arg(&format!("-j{}", cpu_number))
                .output()
                .expect(&format!("make -C {:?} failed", source_path));
            if !result.status.success() {
                let stderr = format!(
                    "* stderr:\n{}",
                    String::from_utf8(result.stderr).expect("invalid utf8 str from make stderr")
                );
                let stdout = format!(
                    "* stdout:\n{}",
                    String::from_utf8(result.stdout).expect("invalid utf8 str from make stdout")
                );
                panic!("make failed:\n{}", vec![stderr, stdout].join("\n\n"));
            }
        }
    }
    // LINK
    println!("cargo:rustc-link-search=native={}", source_path.to_str().expect("PathBuf to str"));
    #[cfg(not(target_os = "windows"))]
    {
        for path in SEARCH_PATHS {
            println!("cargo:rustc-link-search=native={}", {
                source_path.join(path).to_str().expect("PathBuf as str")
            });
        }
    }
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rustc-link-search=native={}", "D:\\Robert\\repos\\vcpkg\\installed\\x64-windows-static\\lib");
        println!("cargo:rustc-link-search=static={}", "D:\\Robert\\repos\\vcpkg\\installed\\x64-windows-static\\lib");
        
        println!("cargo:rustc-link-lib={}", "Bcrypt");
        println!("cargo:rustc-link-lib={}", "Secur32");
        println!("cargo:rustc-link-lib={}", "Ole32");
        println!("cargo:rustc-link-lib={}", "User32");
    }
    
    for (name, _) in STATIC_LIBS {
        println!("cargo:rustc-link-lib=static={}", name);
    }
    // CODEGEN
    {
        // SETUP
        println!("rerun-if-changed=headers");
        let ffmpeg_headers = std::fs::read("headers").expect("unable to read headers file");
        let ffmpeg_headers = String::from_utf8(ffmpeg_headers).expect("invalid utf8 file");
        let ffmpeg_headers = ffmpeg_headers
            .lines()
            .collect::<Vec<&str>>();
        assert!(
            ffmpeg_headers
                .iter()
                .map(|x| x.trim())
                .all(|x| !x.is_empty())
        );
        
        let gen_file_name = "bindings_ffmpeg.rs";
        let ignored_macros = IgnoreMacros(HashSet::from_iter(vec![
            String::from("FP_INFINITE"),
            String::from("FP_NAN"),
            String::from("FP_NORMAL"),
            String::from("FP_SUBNORMAL"),
            String::from("FP_ZERO"),
            String::from("IPPORT_RESERVED"),
        ]));
        let mut skip_codegen = out_path.join(gen_file_name).exists();
        if has_env_var_with_value("FFDEV2", "2") {
            skip_codegen = false;
        }
        // CONFIG
        if !skip_codegen {
            let codegen = bindgen::Builder::default();
            let codegen = codegen.clang_arg(format!("-I{}", source_path.to_str().expect("PathBuf to str")));
            let mut missing = Vec::new();
            let codegen = ffmpeg_headers
                .iter()
                .fold(codegen, |codegen: bindgen::Builder, path: &&str| -> bindgen::Builder {
                    let path: &str = path.clone();
                    let path: PathBuf = source_path.join(path);
                    let path: &str = path.to_str().expect("PathBuf to str");
                    if !PathBuf::from(path).exists() {
                        missing.push(String::from(path));
                        codegen
                    } else {
                        codegen.header(path)
                    }
                });
            #[cfg(target_os = "windows")]
            let codegen = {
                codegen.clang_arg(format!("-isystem{}", source_path.join("compat").join("atomics").join("win32").to_str().unwrap()))
            };
            if !missing.is_empty() {
                panic!("missing headers: {:#?}", missing);
            }
            // RUN
            codegen
                .parse_callbacks(Box::new(ignored_macros.clone()))
                .layout_tests(false)
                .rustfmt_bindings(true)
                .detect_include_paths(true)
                .generate_comments(true)
                .whitelist_function("av.*")
                .whitelist_type("AV.*")
                .generate()
                .expect("Unable to generate bindings")
                .write_to_file(out_path.join(gen_file_name))
                .expect("Couldn't write bindings!");
        }
    }
    // COMPILE CBITS
    cc::Build::new()
        .include({
            source_path.to_str().expect("PathBuf to str")
        })
        .file("cbits/defs.c")
        .file("cbits/img_utils.c")
        .compile("cbits");
}

///////////////////////////////////////////////////////////////////////////////
// MAIN
///////////////////////////////////////////////////////////////////////////////

fn main() {
    
    build();
}
