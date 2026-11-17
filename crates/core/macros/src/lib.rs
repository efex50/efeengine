use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::{parse_macro_input, spanned::Spanned, Attribute, Data, DeriveInput, Fields, ItemFn, LitStr};

#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // 1. Parse the user's function
    let mut input_fn = parse_macro_input!(item as ItemFn);

    // 2. Rename the user's function so we can call it from our wrappers
    // We change "main" to "__engine_user_main"
    let user_main_ident = syn::Ident::new("__engine_user_main", input_fn.sig.ident.span());
    input_fn.sig.ident = user_main_ident.clone();

    // 3. Generate the wrapper code
    let expanded = quote! {
        // Emit the user's original function (renamed)
        #input_fn

        // --- Native Entry Point ---
        // `run` is what the example's `src/main.rs` calls (`fn main() { my_game::run() }`), so
        // the lib and the bin don't share a source file. `main` keeps a bin built straight from
        // this file working.
        #[cfg(not(target_arch = "wasm32"))]
        pub fn run(){
            __engine_user_main()
        }
        #[cfg(not(target_arch = "wasm32"))]
        #[allow(dead_code)]
        fn main(){
            run()
        }

        // --- WASM Entry Point ---
        #[cfg(target_arch = "wasm32")]
        use wasm_bindgen::prelude::*;
        #[cfg(target_arch = "wasm32")]
        #[wasm_bindgen]
        pub fn __engine_start(config: JsValue) -> Result<(), JsValue>{
            // parse the optional startup config, then run the game
            efeengine_core::engine::config::__start_from_js(config)?;
            __engine_user_main();
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        #[wasm_bindgen(start)]
        pub fn main(){
            // init the game but not start it for workers to not start


        }
    };

    TokenStream::from(expanded)
}

/// `#[derive(ScriptType)]`: implements `efeengine_core::engine::types::ScriptType` for a
/// `#[repr(C)]` struct with named fields, describing its byte layout as an
/// `efevm::StructDesc` (offsets from `offset_of!`, field types from `ScriptField`).
///
/// Attributes:
/// - `#[script(name = "Other")]` on the struct: the stable name (default: the Rust name).
/// - `#[script(readonly)]` on the struct: scripts can read the component but not write it.
/// - `#[script(entity)]` on a `u64` (or `[u64; N]`) field: the field holds entity handles.
///
/// `///` doc comments of the struct and its fields become the `doc` strings.
#[proc_macro_derive(ScriptType, attributes(script))]
pub fn derive_script_type(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match script_type(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn script_type(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(input.generics.span(), "ScriptType cannot be derived for a generic type"));
    }
    let data = match &input.data {
        Data::Struct(s) => s,
        Data::Enum(e) => return Err(syn::Error::new(e.enum_token.span, "ScriptType needs a struct with named fields, not an enum")),
        Data::Union(u) => return Err(syn::Error::new(u.union_token.span, "ScriptType needs a struct with named fields, not a union")),
    };
    let fields = match &data.fields {
        Fields::Named(f) => f,
        Fields::Unnamed(f) => return Err(syn::Error::new(f.span(), "ScriptType needs named fields; tuple structs are not supported")),
        Fields::Unit => return Err(syn::Error::new(ident.span(), "ScriptType needs named fields; unit structs are not supported")),
    };
    if !has_repr_c(&input.attrs)? {
        return Err(syn::Error::new(ident.span(), "ScriptType needs `#[repr(C)]` so the field layout is fixed"));
    }

    let mut name = ident.to_string();
    let mut readonly = false;
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("script")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let lit: LitStr = meta.value()?.parse()?;
                name = lit.value();
                Ok(())
            } else if meta.path.is_ident("readonly") {
                readonly = true;
                Ok(())
            } else {
                Err(meta.error("unknown struct attribute; expected `#[script(name = \"...\")]` or `#[script(readonly)]`"))
            }
        })?;
    }
    let struct_doc = doc_string(&input.attrs);

    let mut pushes = Vec::new();
    for f in &fields.named {
        let fname = f.ident.as_ref().expect("named field");
        let fname_str = fname.to_string();
        let ty = &f.ty;
        let mut entity = false;
        for attr in f.attrs.iter().filter(|a| a.path().is_ident("script")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("entity") {
                    entity = true;
                    Ok(())
                } else {
                    Err(meta.error("unknown field attribute; expected `#[script(entity)]`"))
                }
            })?;
        }
        let doc = doc_string(&f.attrs);
        let prim = if entity {
            quote_spanned! {ty.span()=>
                {
                    const _: () = ::core::assert!(
                        ::core::matches!(<#ty as ::efeengine_core::engine::types::ScriptField>::PRIM, ::efeengine_core::efevm::Prim::U64),
                        "#[script(entity)] needs a u64 field (or an array of u64)"
                    );
                    ::efeengine_core::efevm::Prim::Entity
                }
            }
        } else {
            quote_spanned! {ty.span()=> <#ty as ::efeengine_core::engine::types::ScriptField>::PRIM }
        };
        let count = quote_spanned! {ty.span()=> <#ty as ::efeengine_core::engine::types::ScriptField>::COUNT };
        pushes.push(quote! {
            d = d.field(
                ::efeengine_core::efevm::FieldDesc::new(
                    #fname_str,
                    ::core::mem::offset_of!(Self, #fname) as u32,
                    #prim,
                    #count,
                )
                .with_doc(#doc),
            );
        });
    }

    Ok(quote! {
        impl ::efeengine_core::engine::types::ScriptType for #ident {
            const NAME: &'static str = #name;
            fn layout() -> ::efeengine_core::efevm::StructDesc {
                let mut d = ::efeengine_core::efevm::StructDesc::new(
                    #name,
                    ::core::mem::size_of::<Self>() as u32,
                    ::core::mem::align_of::<Self>() as u32,
                )
                .doc(#struct_doc)
                .readonly(#readonly);
                #(#pushes)*
                d
            }
        }
    })
}

/// `true` if `#[repr(...)]` lists `C` (other hints such as `align(16)` may follow).
fn has_repr_c(attrs: &[Attribute]) -> syn::Result<bool> {
    let mut c = false;
    for attr in attrs.iter().filter(|a| a.path().is_ident("repr")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("C") {
                c = true;
            } else if meta.input.peek(syn::token::Paren) {
                // e.g. align(16): skip its arguments
                let _content;
                syn::parenthesized!(_content in meta.input);
            }
            Ok(())
        })?;
    }
    Ok(c)
}

/// `///` lines, each trimmed, joined with `\n`, trimmed as a whole.
fn doc_string(attrs: &[Attribute]) -> String {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Some(s.value().trim().to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    lines.join("\n").trim().to_string()
}

/// Options of `#[script_component(..)]` and `#[component(..)]`.
#[derive(Default)]
struct CompOpts {
    name: Option<String>,
    tracked: bool,
    readonly: bool,
}

impl CompOpts {
    fn parse_meta(&mut self, meta: syn::meta::ParseNestedMeta, allow_name: bool, readonly_ok: bool) -> syn::Result<()> {
        if allow_name && meta.path.is_ident("name") {
            let lit: LitStr = meta.value()?.parse()?;
            self.name = Some(lit.value());
            Ok(())
        } else if meta.path.is_ident("tracked") {
            self.tracked = true;
            Ok(())
        } else if allow_name && readonly_ok && meta.path.is_ident("readonly") {
            self.readonly = true;
            Ok(())
        } else if allow_name {
            Err(meta.error("unknown option; expected `name = \"...\"`, `tracked` or `readonly`"))
        } else {
            Err(meta.error("unknown option; expected `tracked`"))
        }
    }
}

/// `#[script_component]` / `#[script_component(name = "Hp", tracked, readonly)]` on a struct with named
/// fields: makes it a script-visible component. It adds `#[repr(C)]`, derives
/// `Clone, Copy, Pod, Zeroable, ScriptType` (a game needs no `bytemuck` dependency) and
/// implements `ComponentInfo`, so `engine.register::<T>()` registers it. Your own
/// `#[derive(..)]` may list any of those traits; they are not derived twice.
///
/// `readonly`: scripts can read the component (`engine.world.get`, queries) but
/// `engine.world.set/insert/remove` on it fail with status 5.
///
/// Rules are those of `ScriptType`: no generics, named fields, no `#[repr]` of your own.
/// (Compile-fail examples live in the docs of `ComponentInfo` in `efeengine-core`.)
#[proc_macro_attribute]
pub fn script_component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut opts = CompOpts::default();
    let parser = syn::meta::parser(|meta| opts.parse_meta(meta, true, true));
    parse_macro_input!(attr with parser);
    let input = parse_macro_input!(item as DeriveInput);
    match script_component_impl(opts, input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn script_component_impl(opts: CompOpts, mut input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = input.ident.clone();
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(input.generics.span(), "#[script_component] cannot be used on a generic type"));
    }
    match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(_) => {}
            Fields::Unnamed(f) => return Err(syn::Error::new(f.span(), "#[script_component] needs named fields; tuple structs are not supported")),
            Fields::Unit => return Err(syn::Error::new(ident.span(), "#[script_component] needs named fields; unit structs are not supported")),
        },
        Data::Enum(e) => return Err(syn::Error::new(e.enum_token.span, "#[script_component] needs a struct with named fields, not an enum")),
        Data::Union(u) => return Err(syn::Error::new(u.union_token.span, "#[script_component] needs a struct with named fields, not a union")),
    }
    if let Some(r) = input.attrs.iter().find(|a| a.path().is_ident("repr")) {
        return Err(syn::Error::new(r.span(), "#[script_component] adds `#[repr(C)]` itself; remove your `#[repr]`"));
    }

    // Drop traits we derive ourselves from the user's own derive lists.
    const OURS: [&str; 5] = ["Clone", "Copy", "Pod", "Zeroable", "ScriptType"];
    let mut attrs = Vec::new();
    for attr in std::mem::take(&mut input.attrs) {
        if !attr.path().is_ident("derive") {
            attrs.push(attr);
            continue;
        }
        let paths = attr.parse_args_with(syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)?;
        let kept: Vec<_> = paths
            .into_iter()
            .filter(|p| !p.segments.last().is_some_and(|s| OURS.iter().any(|o| s.ident == o)))
            .collect();
        if !kept.is_empty() {
            attrs.push(syn::parse_quote!(#[derive(#(#kept),*)]));
        }
    }
    input.attrs = attrs;

    let name = opts.name.clone().unwrap_or_else(|| ident.to_string());
    let script_name = opts.name.as_ref().map(|n| quote!(#[script(name = #n)]));
    let script_readonly = opts.readonly.then(|| quote!(#[script(readonly)]));
    let tracked = opts.tracked;
    let info = component_info(&ident, &input.generics, &name, tracked, true);
    Ok(quote! {
        #[repr(C)]
        #[derive(
            ::core::clone::Clone,
            ::core::marker::Copy,
            ::efeengine_core::bytemuck::Pod,
            ::efeengine_core::bytemuck::Zeroable,
            ::efeengine_core::prelude::ScriptType,
        )]
        #[bytemuck(crate = "::efeengine_core::bytemuck")]
        #script_name
        #script_readonly
        #input
        #info
    })
}

/// `#[derive(Component)]` with optional `#[component(name = "..", tracked)]`: implements
/// `ComponentInfo` for a Rust-only component (any type: tuple structs, enums, generics). It is
/// optional (every `Send + Sync + 'static` type is a component); use it to get change tracking
/// registered by `engine.register::<T>()`.
#[proc_macro_derive(Component, attributes(component))]
pub fn derive_component(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let mut opts = CompOpts::default();
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("component")) {
        if let Err(e) = attr.parse_nested_meta(|meta| opts.parse_meta(meta, true, false)) {
            return e.to_compile_error().into();
        }
    }
    let name = opts.name.clone().unwrap_or_else(|| input.ident.to_string());
    component_info(&input.ident, &input.generics, &name, opts.tracked, false).into()
}

fn component_info(ident: &syn::Ident, generics: &syn::Generics, name: &str, tracked: bool, script: bool) -> proc_macro2::TokenStream {
    let (ig, tg, wc) = generics.split_for_impl();
    let register = if script {
        quote! {
            let owner = engine.current_owner();
            let id = engine.register_component_owned::<Self>(&owner)?;
            if <Self as ::efeengine_core::engine::types::ComponentInfo>::TRACKED {
                engine.track_changes::<Self>();
            }
            ::core::result::Result::Ok(::core::option::Option::Some(id))
        }
    } else {
        quote! {
            engine.check_registration_open(::core::any::type_name::<Self>())?;
            if <Self as ::efeengine_core::engine::types::ComponentInfo>::TRACKED {
                engine.track_changes::<Self>();
            }
            ::core::result::Result::Ok(::core::option::Option::None)
        }
    };
    quote! {
        impl #ig ::efeengine_core::engine::types::ComponentInfo for #ident #tg #wc {
            const NAME: &'static str = #name;
            const TRACKED: bool = #tracked;
            const SCRIPT: bool = #script;
            fn register(engine: &mut ::efeengine_core::engine::Engine) -> ::core::result::Result<::core::option::Option<u32>, ::efeengine_core::engine::types::TypeError> {
                #register
            }
        }
    }
}
