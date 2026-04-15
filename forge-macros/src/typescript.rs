use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

/// Expands `#[derive(forge::TS)]` to register the type for TypeScript export.
///
/// The type MUST also derive `ts_rs::TS` (or have it derived by another macro).
/// This macro only adds the `inventory::submit!` registration.
pub fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let name = ident.to_string();

    Ok(quote! {
        ::inventory::submit! {
            ::forge::typescript::TsType {
                name: #name,
                export_fn: |dir| <#ident as ::ts_rs::TS>::export_all_to(dir),
            }
        }
    })
}
