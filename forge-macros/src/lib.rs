use proc_macro::TokenStream;

mod app_enum;
mod common;
mod model;
mod openapi;
mod projection;
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
    expand(input, app_enum::expand)
}

#[proc_macro_derive(Validate, attributes(validate))]
pub fn derive_validate(input: TokenStream) -> TokenStream {
    expand(input, validate::expand)
}

#[proc_macro_derive(ApiSchema)]
pub fn derive_api_schema(input: TokenStream) -> TokenStream {
    expand(input, openapi::expand)
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
