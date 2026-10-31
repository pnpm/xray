use super::parse;
use std::path::Path;

#[test]
fn parses_static_sass_references_and_ignores_comments_and_interpolation() {
    let source = "/* @use 'fake'; */\n@use 'sass:math';\n@forward '@material/base/mixins' as m-*;\n@use './local';\n@use '#{$dynamic}';";
    let refs = parse(Path::new("test.scss"), source).unwrap();
    assert_eq!(
        refs.iter().map(|reference| reference.specifier.as_str()).collect::<Vec<_>>(),
        ["@material/base/mixins", "./local"],
    );
    assert_eq!(&source[refs[0].offset..=refs[0].offset], "'");
}
#[test]
fn parses_css_and_less_imports_but_not_external_urls() {
    let css = parse(
        Path::new("test.css"),
        "@import url('pkg/base.css'); @import 'https://example.com/a.css';",
    )
    .unwrap();
    assert_eq!(css.len(), 1);
    assert_eq!(css[0].specifier, "pkg/base.css");
    let less = parse(Path::new("test.less"), "@import (reference) 'pkg/styles.less';").unwrap();
    assert_eq!(less[0].specifier, "pkg/styles.less");
}
#[test]
fn reports_invalid_stylesheets() {
    assert!(parse(Path::new("test.scss"), "@use ;").is_err());
}

#[test]
fn parses_legacy_ie_star_properties_without_changing_reference_offsets() {
    let source = "/* *fake: x; */\n@import 'pkg/style.css';\na { content: '*fake: x'; *zoom: 1; *color: red; }";
    let references = parse(Path::new("legacy.css"), source).unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].specifier, "pkg/style.css");
    assert_eq!(references[0].offset, source.find("'pkg/style.css'").unwrap());
    assert!(parse(Path::new("legacy.css"), "a { *zoom: ; @import ; }").is_err());
}

#[test]
fn parses_interpolated_custom_property_without_losing_imports() {
    let source = r#"@use 'real-package'; a { --icon: url("data:image/svg+xml,stroke='#{to-rgb($white)}'%3E"); } @forward 'other-package';"#;
    let refs = parse(Path::new("test.scss"), source).unwrap();
    assert_eq!(
        refs.iter().map(|item| item.specifier.as_str()).collect::<Vec<_>>(),
        ["real-package", "other-package"],
    );
    assert_eq!(refs[1].offset, source.find("'other-package'").unwrap());
    assert!(parse(Path::new("test.scss"), r##"a { --x: "#{oops(}"; } @use ;"##).is_err());
}
