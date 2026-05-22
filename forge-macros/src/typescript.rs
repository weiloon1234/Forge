use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

/// Expands `#[derive(forge::TS)]` to register the type for TypeScript export.
pub fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    reject_direct_ts_rs_export(&input.attrs)?;

    let ident = &input.ident;
    let name = ident.to_string();

    Ok(quote! {
        ::forge::inventory::submit! {
            ::forge::typescript::TsType {
                name: #name,
                export_fn: |dir| <#ident as ::forge::ts_rs::TS>::export_all_to(dir),
                output_path_fn: || <#ident as ::forge::ts_rs::TS>::output_path(),
            }
        }
    })
}

fn reject_direct_ts_rs_export(attrs: &[syn::Attribute]) -> syn::Result<()> {
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("ts")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("export") {
                return Err(meta.error(
                    "Forge owns TypeScript export output through `types:export`; remove `export` and run the Forge exporter instead",
                ));
            }

            if meta.input.peek(syn::Token![=]) {
                let value = meta.value()?;
                let _: syn::Expr = value.parse()?;
            } else if meta.input.peek(syn::token::Paren) {
                let content;
                syn::parenthesized!(content in meta.input);
                let _: TokenStream = content.parse()?;
            }

            Ok(())
        })?;
    }

    Ok(())
}

/// Additional registration for AppEnum types — includes runtime metadata.
pub fn expand_app_enum(input: &DeriveInput) -> TokenStream {
    let ident = &input.ident;
    let name = ident.to_string();

    quote! {
        ::forge::inventory::submit! {
            ::forge::typescript::TsAppEnum {
                name: #name,
                meta_fn: || <#ident as ::forge::ForgeAppEnum>::meta(),
            }
        }
    }
}
