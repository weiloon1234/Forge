use proc_macro::TokenStream;

mod app_enum;
mod common;
mod model;
mod openapi;
mod projection;
mod typescript;
mod validate;

#[proc_macro_derive(Model, attributes(forge))]
pub fn derive_model(input: TokenStream) -> TokenStream {
    expand(input, model::expand)
}

#[proc_macro_derive(Projection, attributes(forge))]
pub fn derive_projection(input: TokenStream) -> TokenStream {
    expand(input, projection::expand)
}

#[proc_macro_derive(AppEnum, attributes(forge))]
pub fn derive_app_enum(input: TokenStream) -> TokenStream {
    expand_with_ts(input, app_enum::expand)
}

#[proc_macro_derive(Validate, attributes(validate))]
pub fn derive_validate(input: TokenStream) -> TokenStream {
    expand(input, validate::expand)
}

#[proc_macro_derive(ApiSchema)]
pub fn derive_api_schema(input: TokenStream) -> TokenStream {
    expand_with_ts(input, openapi::expand)
}

#[proc_macro_derive(TS)]
pub fn derive_ts(input: TokenStream) -> TokenStream {
    expand(input, typescript::expand)
}

fn expand(
    input: TokenStream,
    f: fn(syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream>,
) -> TokenStream {
    match syn::parse(input).and_then(f) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Like `expand`, but also appends TypeScript inventory registration.
fn expand_with_ts(
    input: TokenStream,
    f: fn(syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream>,
) -> TokenStream {
    match syn::parse::<syn::DeriveInput>(input) {
        Ok(parsed) => {
            let ts_tokens = typescript::expand(parsed.clone());
            match f(parsed) {
                Ok(main_tokens) => {
                    let ts = ts_tokens.unwrap_or_default();
                    let combined = quote::quote! {
                        #main_tokens
                        #ts
                    };
                    combined.into()
                }
                Err(error) => error.to_compile_error().into(),
            }
        }
        Err(error) => error.to_compile_error().into(),
    }
}
