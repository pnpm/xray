use super::collect;
use std::{path::Path, process::Command};

fn write(path: &Path, value: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value).unwrap();
}

#[test]
fn compiler_requires_installed_runtime_and_unambiguous_matching_types() {
    let Ok(typescript) = std::env::var("XRAY_TYPESCRIPT") else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let owner = root.join("node_modules/example");
    let runtime = root.join("node_modules/foo");
    let provider = root.join("store/node_modules/@types/foo");
    write(&root.join("package.json"), r#"{"private":true}"#);
    write(
        &root.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"types":[],"moduleResolution":"node","noEmit":true},"files":["consumer.ts"]}"#,
    );
    write(&root.join("consumer.ts"), "import {x} from 'example'; void x;");
    write(
        &owner.join("package.json"),
        r#"{"name":"example","version":"1.0.0","types":"index.d.ts","dependencies":{"foo":"1.0.0"}}"#,
    );
    write(&owner.join("index.d.ts"), "import { F } from 'foo'; export declare const x: F;");
    write(&runtime.join("package.json"), r#"{"name":"foo","version":"1.0.0","main":"index.js"}"#);
    write(&runtime.join("index.js"), "module.exports = 1;");
    write(
        &provider.join("package.json"),
        r#"{"name":"@types/foo","version":"1.2.3","types":"index.d.ts"}"#,
    );
    write(&provider.join("index.d.ts"), "export interface F {ok: true}");
    let status = Command::new("node")
        .args(["-e", "require('node:fs').symlinkSync(process.argv[1],process.argv[2],'junction')"])
        .arg(typescript)
        .arg(root.join("node_modules/typescript"))
        .status()
        .unwrap();
    assert!(status.success());
    let packages = vec![owner.clone(), runtime.clone(), provider.clone()];
    let proposals = collect(root, &packages).unwrap();
    assert_eq!(proposals.len(), 1);
    assert!(proposals[0].configuration.contains("'@types/foo': '1.2.3'"));
    write(&root.join("tsconfig.json"), r#"{"files":[],"references":[{"path":"./app"}]}"#);
    write(
        &root.join("app/tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"types":[],"moduleResolution":"node","noEmit":true},"files":["consumer.ts"]}"#,
    );
    write(&root.join("app/consumer.ts"), "import {x} from 'example'; void x;");
    assert_eq!(collect(root, &packages).unwrap().len(), 1);
    assert!(collect(root, &[owner.clone(), runtime.clone()]).unwrap().is_empty());
    write(&runtime.join("index.d.ts"), "export interface F {ok: true}");
    assert!(collect(root, &packages).unwrap().is_empty());
    std::fs::remove_file(runtime.join("index.d.ts")).unwrap();
    std::fs::remove_file(runtime.join("index.js")).unwrap();
    assert!(collect(root, &packages).unwrap().is_empty());
    write(&runtime.join("index.js"), "module.exports = 1;");
    let duplicate = root.join("other/node_modules/@types/foo");
    write(
        &duplicate.join("package.json"),
        r#"{"name":"@types/foo","version":"2.0.0","types":"index.d.ts"}"#,
    );
    write(&duplicate.join("index.d.ts"), "export interface F {ok: true}");
    assert!(collect(root, &[owner, runtime, provider, duplicate]).unwrap().is_empty());
}

#[test]
fn compiler_proves_undeclared_deep_type_import_with_installed_provider() {
    let Ok(typescript) = std::env::var("XRAY_TYPESCRIPT") else { return };
    let project = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = project.path();
    let owner = store.path().join("node_modules/example");
    let provider = store.path().join("other/node_modules/dep");
    write(&root.join("package.json"), r#"{"private":true}"#);
    write(
        &root.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"types":[],"moduleResolution":"node","noEmit":true},"files":["consumer.ts"]}"#,
    );
    write(&root.join("consumer.ts"), "import type { X } from 'example'; let x: X; void x!;");
    write(
        &owner.join("package.json"),
        r#"{"name":"example","version":"1.0.0","types":"index.d.ts"}"#,
    );
    write(&owner.join("index.d.ts"), "export type { X } from 'dep/deep';");
    write(&provider.join("package.json"), r#"{"name":"dep","version":"1.2.3"}"#);
    write(&provider.join("deep.d.ts"), "export interface X {ok: true}");
    std::fs::create_dir_all(root.join("node_modules")).unwrap();
    for (from, to) in [
        (std::path::PathBuf::from(typescript), root.join("node_modules/typescript")),
        (owner.clone(), root.join("node_modules/example")),
    ] {
        let status = Command::new("node")
            .args([
                "-e",
                "require('node:fs').symlinkSync(process.argv[1],process.argv[2],'junction')",
            ])
            .arg(from)
            .arg(to)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let packages = vec![owner.clone(), provider.clone()];
    let proposals = collect(root, &packages).unwrap();
    assert_eq!(proposals.len(), 1);
    assert!(proposals[0].configuration.contains("'dep': '*'"));
    assert!(collect(root, std::slice::from_ref(&owner)).unwrap().is_empty());
    let other = store.path().join("third/node_modules/dep");
    write(&other.join("package.json"), r#"{"name":"dep","version":"2.0.0"}"#);
    write(&other.join("deep.d.ts"), "export interface X {ok: false}");
    let ambiguous = vec![owner, provider.clone(), other];
    assert!(collect(root, &ambiguous).unwrap().is_empty());
    let status = Command::new("node")
        .args(["-e", "require('node:fs').symlinkSync(process.argv[1],process.argv[2],'junction')"])
        .arg(&provider)
        .arg(root.join("node_modules/dep"))
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(collect(root, &ambiguous).unwrap().len(), 1);
    std::fs::remove_file(provider.join("deep.d.ts")).unwrap();
    assert!(collect(root, &packages).unwrap().is_empty());
}
