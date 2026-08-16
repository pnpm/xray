#[cfg(test)]
mod tests;

use crate::scan::{Origin, Requirement};
use std::collections::HashSet;

/// The package that has to be installed for a requirement to resolve, or `None`
/// when the package already declared something that satisfies it.
pub fn missing_package(
    requirement: &Requirement,
    origin: Origin,
    declared: &HashSet<&str>,
) -> Option<String> {
    match requirement {
        // In a type position a bare specifier can be satisfied by a types
        // package that declares the module ambiently, the way `@types/estree`
        // declares `estree`. Executing code needs the package itself: a
        // declaration file carries no implementation.
        Requirement::Module(specifier) => {
            let name = package_name(specifier)?;
            let satisfied = declared.contains(name)
                || (origin == Origin::Types && declared.contains(types_package(name).as_str()));
            (!satisfied).then(|| name.to_string())
        }
        // `/// <reference types="x" />` is satisfied by `@types/x` or by `x`
        // itself when that package ships its own declarations.
        Requirement::TypesReference(name) => {
            let types_package = types_package(name);
            let satisfied =
                declared.contains(name.as_str()) || declared.contains(types_package.as_str());
            (!satisfied).then_some(types_package)
        }
    }
}

/// The package under `@types/` that carries declarations for `name`, following
/// the scope-mangling convention `DefinitelyTyped` publishes under.
pub fn types_package(name: &str) -> String {
    match name.strip_prefix('@') {
        Some(scoped) => format!("@types/{}", scoped.replacen('/', "__", 1)),
        None => format!("@types/{name}"),
    }
}

/// The package a specifier belongs to, or `None` when nothing needs to be
/// installed for it to resolve: relative paths, subpath imports, protocol URLs
/// and Node builtins.
pub fn package_name(specifier: &str) -> Option<&str> {
    if specifier.starts_with(['.', '/', '#']) || specifier.is_empty() {
        return None;
    }
    if let Some((scheme, _)) = specifier.split_once(':') {
        if !scheme.starts_with('@') {
            return None;
        }
    }

    let mut segments = specifier.split('/');
    let first = segments.next()?;
    let name = if first.starts_with('@') {
        let scope_end = first.len() + 1 + segments.next()?.len();
        &specifier[..scope_end]
    } else {
        first
    };

    if NODE_BUILTINS.contains(&name) {
        return None;
    }
    Some(name)
}

const NODE_BUILTINS: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "sea",
    "sqlite",
    "stream",
    "string_decoder",
    "sys",
    "test",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];
