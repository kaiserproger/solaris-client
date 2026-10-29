//! Every WGSL shader the client builds, exactly as it builds it, must parse,
//! validate, resolve its pipeline overrides and translate for Vulkan (SPIR-V),
//! DX12 (HLSL) and Metal (MSL) with the naga wgpu uses. No GPU needed.
use std::path::Path;

/// (label, runtime source, override constants a pipeline sets).
fn shaders() -> Vec<(&'static str, String, Vec<(&'static str, f64)>)> {
    vec![
        // `shader.wgsl` + `atmosphere.wgsl`; specializations keep their defaults.
        (
            "shader.wgsl+atmosphere.wgsl",
            super::graphics::main_shader_source(),
            vec![],
        ),
        (
            "far.wgsl",
            include_str!("far.wgsl").into(),
            vec![("MAX_FACES", super::far::MAX_FACES as f64)],
        ),
        // A typical program's register count; the source differs only in that literal.
        (
            "far_generate.wgsl",
            super::far_generate::shader_source(64),
            vec![],
        ),
        ("hiz.wgsl", include_str!("hiz.wgsl").into(), vec![]),
        ("mdi.wgsl", include_str!("mdi.wgsl").into(), vec![]),
        ("outline.wgsl", include_str!("outline.wgsl").into(), vec![]),
        ("post.wgsl", include_str!("post.wgsl").into(), vec![]),
    ]
}

fn check(label: &str, source: &str, constants: &[(&str, f64)]) -> Result<(), String> {
    use naga::valid::{Capabilities, ValidationFlags, Validator};
    let module = naga::front::wgsl::parse_str(source)
        .map_err(|e| format!("{label}: parse\n{}", e.emit_to_string(source)))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .map_err(|e| format!("{label}: validate\n{}", e.emit_to_string(source)))?;
    let constants: naga::back::PipelineConstants = constants
        .iter()
        .map(|(name, value)| (name.to_string(), *value))
        .collect();
    let (module, info) =
        naga::back::pipeline_constants::process_overrides(&module, &info, None, &constants)
            .map_err(|e| format!("{label}: overrides {e}"))?;
    naga::back::spv::write_vec(&module, &info, &Default::default(), None)
        .map_err(|e| format!("{label}: SPIR-V {e}"))?;
    let mut hlsl = String::new();
    naga::back::hlsl::Writer::new(&mut hlsl, &Default::default(), &Default::default())
        .write(&module, &info, None)
        .map_err(|e| format!("{label}: HLSL {e}"))?;
    naga::back::msl::write_string(&module, &info, &Default::default(), &Default::default())
        .map_err(|e| format!("{label}: MSL {e}"))?;
    Ok(())
}

#[test]
fn every_runtime_shader_validates_and_translates_for_each_backend() {
    let failures: Vec<String> = shaders()
        .iter()
        .filter_map(|(label, source, constants)| check(label, source, constants).err())
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// A new `.wgsl` file must join [`shaders`], so it is validated too.
#[test]
fn every_wgsl_file_is_validated() {
    let shaders = shaders();
    let covered: Vec<&str> = shaders
        .iter()
        .flat_map(|(label, ..)| label.split('+'))
        .collect();
    let mut missing = Vec::new();
    let mut stack = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "wgsl") {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if !covered.contains(&name.as_str()) {
                    missing.push(name);
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "add to wgsl_validation_tests::shaders: {missing:?}"
    );
}
