use std::{fs, path::Path};

pub fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

pub fn manifest(name: &str, extra: &str) -> String {
    format!(r#"{{"name":"{name}","version":"1.0.0"{extra}}}"#)
}

/// A project whose packages really live elsewhere, the shape a global virtual
/// store produces: `node_modules` holds links, and each store entry holds the
/// package next to the dependencies it may reach.
///
/// `app` declares an `exports` map, so only what it points at is the package's
/// surface: it leaks one devDependency into its declarations, requires another
/// from executable code, bundles a copy of something else, and ships a test
/// nothing reaches. `helper` declares the dependency it imports.
#[cfg(unix)]
pub fn global_virtual_store() -> tempfile::TempDir {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let store = root.path().join("store");

    let app = store.join("app/node_modules/app");
    write(
        &app.join("package.json"),
        &manifest(
            "app",
            r#","exports":{".":"./index.js"},"types":"index.d.ts","devDependencies":{"ghost":"^1","runtime-ghost":"^3"}"#,
        ),
    );
    write(&app.join("index.d.ts"), "import type { G } from 'ghost';\nexport type { G };\n");
    write(
        &app.join("index.js"),
        "const r = require('runtime-ghost');\nconst l = require('./lib/helper');\nmodule.exports = [r, l];\n",
    );
    // Reached from the entry, so what it imports is the package's own business.
    write(&app.join("lib/helper.js"), "module.exports = require('reached-ghost');\n");
    // Shipped in the tarball but reachable from nothing: its imports belong to
    // whoever runs the tests, not to whoever installs the package.
    write(&app.join("test/spec.js"), "require('test-only-ghost');\n");
    write(&app.join("node_modules/vendored/index.d.ts"), "import 'not-yours';\n");

    let helper = store.join("helper/node_modules/helper");
    write(
        &helper.join("package.json"),
        &manifest("helper", r#","dependencies":{"declared":"^2"}"#),
    );
    write(&helper.join("index.d.ts"), "import type { D } from 'declared';\nexport type { D };\n");

    symlink(&helper, store.join("app/node_modules/helper")).unwrap();
    fs::create_dir_all(project.join("node_modules")).unwrap();
    symlink(&app, project.join("node_modules/app")).unwrap();

    root
}
