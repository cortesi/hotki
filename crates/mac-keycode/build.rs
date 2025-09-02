use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{Ident, LitInt};

fn to_variant(name: &str) -> Ident {
    // Strip "kVK_" prefix
    let no_prefix = name.strip_prefix("kVK_").unwrap_or(name);
    // Split into parts and drop initial ANSI segment if present
    let mut parts: Vec<&str> = no_prefix.split('_').filter(|p| !p.is_empty()).collect();
    if matches!(parts.first().copied(), Some("ANSI")) {
        parts.remove(0);
    }

    // Special-case a single digit
    if parts.len() == 1 && parts[0].len() == 1 && parts[0].chars().all(|c| c.is_ascii_digit()) {
        let d = parts[0];
        return format_ident!("Digit{}", d);
    }

    // CamelCase join, preserving inner casing of each part except ensuring first char uppercase
    let mut out = String::new();
    for part in parts {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
        }
        for ch in chars {
            out.push(ch);
        }
    }
    format_ident!("{}", out)
}

fn parse_value_literal(lit: &str) -> Result<u16> {
    let s = lit.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let v = u16::from_str_radix(hex, 16)
            .with_context(|| format!("invalid hex value literal: {}", lit))?;
        Ok(v)
    } else if let Some(oct) = s.strip_prefix('0') {
        if oct.is_empty() {
            Ok(0)
        } else {
            let v = u16::from_str_radix(oct, 8)
                .with_context(|| format!("invalid octal value literal: {}", lit))?;
            Ok(v)
        }
    } else {
        let v: u16 = s
            .parse()
            .with_context(|| format!("invalid decimal value literal: {}", lit))?;
        Ok(v)
    }
}

fn read_keycodes(data_dir: &Path) -> Result<Vec<(String, String, u16)>> {
    let candidates = [
        data_dir.join("keycodes.txt"),
        data_dir.join("keycodes"),
        PathBuf::from("data/keycodes.txt"),
        PathBuf::from("data/keycodes"),
    ];

    let path = candidates
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("keycodes data file not found in ./data"))?;

    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read keycodes from {}", path.display()))?;

    let mut out = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let name = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing name at line {}", lineno + 1))?;
        let val = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing value at line {}", lineno + 1))?;
        let parsed = parse_value_literal(val)
            .with_context(|| format!("while parsing value at line {}", lineno + 1))?;
        out.push((name.to_string(), val.to_string(), parsed));
    }
    Ok(out)
}

fn generate_key_rs(crate_dir: &Path) -> Result<PathBuf> {
    let data_dir = crate_dir.join("data");
    let src_dir = crate_dir.join("src");
    let mut entries = read_keycodes(&data_dir)?;

    // Ensure deterministic ordering for all generated items: sort by code, then name.
    entries.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));

    let mut first_by_code: BTreeMap<u16, String> = BTreeMap::new();
    for (name, _lit, code) in &entries {
        first_by_code.entry(*code).or_insert_with(|| name.clone());
    }

    let variants: Vec<proc_macro2::TokenStream> = entries
        .iter()
        .map(|(name, _lit, code)| {
            let vname = to_variant(name);
            let code_lit = LitInt::new(&format!("0x{:X}", code), Span::call_site());
            quote! { #vname = #code_lit }
        })
        .collect();

    let name_match_arms: Vec<proc_macro2::TokenStream> = entries
        .iter()
        .map(|(name, _lit, _)| {
            let vname = to_variant(name);
            let s = syn::LitStr::new(&vname.to_string(), Span::call_site());
            quote! { Key::#vname => #s }
        })
        .collect();

    let from_keycode_arms: Vec<proc_macro2::TokenStream> = first_by_code
        .iter()
        .map(|(code, name)| {
            let vname = to_variant(name);
            let code_lit = LitInt::new(&format!("0x{:X}", code), Span::call_site());
            quote! { #code_lit => Some(Key::#vname) }
        })
        .collect();

    let from_name_arms: Vec<proc_macro2::TokenStream> = entries
        .iter()
        .map(|(name, _lit, _)| {
            let vname = to_variant(name);
            let s = syn::LitStr::new(&vname.to_string().to_ascii_lowercase(), Span::call_site());
            quote! { #s => Some(Key::#vname) }
        })
        .collect();

    let file_tokens = quote! {
        // @generated
        // This file is generated by build.rs. Do not edit by hand.

        #[repr(u16)]
        #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
        pub enum Key {
            #(#variants,)*
        }

        impl Key {
            /// Returns the canonical name for this key (the enum variant string).
            pub fn name(self) -> &'static str {
                match self {
                    #(#name_match_arms,)*
                }
            }

            /// Looks up a `Key` from a hardware virtual keycode (HIToolbox kVK value).
            pub fn from_keycode(code: u16) -> Option<Self> {
                match code {
                    #(#from_keycode_arms,)*
                    _ => None,
                }
            }

            /// Case-insensitive lookup of a `Key` from its name.
            ///
            /// Accepts strings like "Tab", "tab", or "TAB".
            pub fn from_name(name: &str) -> Option<Self> {
                let lowered = name.to_ascii_lowercase();
                match lowered.as_str() {
                    #(#from_name_arms,)*
                    _ => None,
                }
            }
        }
    };

    let out_path = src_dir.join("key.rs");
    fs::write(&out_path, file_tokens.to_string())
        .with_context(|| format!("failed to write generated file to {}", out_path.display()))?;
    Ok(out_path)
}

fn rustfmt_file(path: &Path) -> Result<()> {
    let status = Command::new("rustfmt")
        .arg("--edition")
        .arg("2024")
        .arg(path.as_os_str())
        .status()
        .context("failed to run rustfmt")?;
    if !status.success() {
        bail!("rustfmt failed with status: {:?}", status);
    }
    Ok(())
}

fn main() -> Result<()> {
    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!(
        "cargo:rerun-if-changed={}",
        crate_dir.join("data/keycodes.txt").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        crate_dir.join("data/keycodes").display()
    );

    let out = generate_key_rs(&crate_dir)?;
    // Format the generated file
    let _ = rustfmt_file(&out);
    Ok(())
}
