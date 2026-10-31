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
        Requirement::Module(specifier) | Requirement::Style(specifier, _) => {
            let specifier = if matches!(requirement, Requirement::Style(_, _)) {
                specifier
                    .strip_prefix("pkg:")
                    .or_else(|| specifier.strip_prefix('~'))
                    .unwrap_or(specifier)
            } else {
                specifier
            };
            let name = if matches!(requirement, Requirement::Style(_, _)) {
                package_name_inner(specifier, false)?
            } else {
                package_name(specifier)?
            };
            let satisfied = declared.contains(name)
                || (origin == Origin::Types && declared.contains(types_package(name).as_str()));
            (!satisfied).then(|| name.to_string())
        }
        // `/// <reference types="x" />` is satisfied by `@types/x` or by `x`
        // itself when that package ships its own declarations. The directive
        // can name a subpath (`vite/client`) or the types package outright
        // (`@types/node`); only the package it lives in has to be installed.
        Requirement::TypesReference(reference) => {
            let name = package_name_inner(reference, false)?;
            if name.starts_with("@types/") {
                return (!declared.contains(name)).then(|| name.to_string());
            }
            let types_package = types_package(name);
            let satisfied = declared.contains(name) || declared.contains(types_package.as_str());
            // A subpath lives in the package that ships it; `@types/` packages
            // mirror only the package root.
            let has_subpath = reference.len() > name.len();
            (!satisfied).then(|| if has_subpath { name.to_string() } else { types_package })
        }
        Requirement::PathReference(_) | Requirement::EsmRelative(_) => None,
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
    package_name_inner(specifier, true)
}

fn package_name_inner(specifier: &str, node_builtins: bool) -> Option<&str> {
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
    let name = if let Some(scope) = first.strip_prefix('@') {
        let package = segments.next()?;
        if !valid_name_segment(scope) || !valid_name_segment(package) {
            return None;
        }
        &specifier[..first.len() + 1 + package.len()]
    } else {
        if !valid_name_segment(first) || first.starts_with('~') {
            return None;
        }
        first
    };

    if node_builtins && NODE_BUILTINS.contains(&specifier) {
        return None;
    }
    Some(name)
}

/// Whether a scope or package name could be published to npm. Bundler path
/// aliases such as `@/components` or `~/utils` look like bare specifiers but name
/// no package, so turning them into dependencies would be wrong. This follows
/// npm's rules loosely: names published before the rules tightened may still
/// carry capitals or `~'!()*`, so only what was never allowed is rejected — an
/// empty segment, a leading `.` or `_`, a leading `~` on an unscoped name, and
/// anything that is not URL-safe.
fn valid_name_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.starts_with(['.', '_'])
        && segment.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-._~!*'()".contains(&byte))
}

const NODE_BUILTINS: &[&str] = &[
    "_http_agent",
    "_http_client",
    "_http_common",
    "_http_incoming",
    "_http_outgoing",
    "_http_server",
    "_stream_duplex",
    "_stream_passthrough",
    "_stream_readable",
    "_stream_transform",
    "_stream_wrap",
    "_tls_common",
    "_tls_wrap",
    "assert",
    "assert/strict",
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
    "dns/promises",
    "domain",
    "events",
    "fs",
    "fs/promises",
    "http",
    "http2",
    "https",
    "inspector",
    "inspector/promises",
    "module",
    "net",
    "os",
    "path",
    "path/posix",
    "path/win32",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "readline/promises",
    "repl",
    "stream",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "string_decoder",
    "sys",
    "timers",
    "timers/promises",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "util/types",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];
