//! The `#[stage]` attribute macro. It parses a wrapped function and emits a
//! marker type, a typed handle, and a small `Stage` impl. All caching, move,
//! and injection logic lives in the `directed` runtime, not here.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{
    FnArg, Ident, ItemFn, Pat, ReturnType, Token, Type,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
};

/// Wrap a function as a graph stage.
///
/// Flags: `lazy`, `cache_last`, `cache_all`, `state(Type)`, and
/// `out(name: Type, ...)` for multiple outputs (the function then returns a
/// tuple in the same order).
#[proc_macro_attribute]
pub fn stage(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    let args = parse_macro_input!(attr as StageArgs);
    match StageConfig::from_args(&input_fn, &args).map(expand) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CacheStrategy {
    None,
    Last,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RefKind {
    Owned,
    Borrowed,
    BorrowedMut,
}

struct InputParam {
    ident: Ident,
    clean: String,
    true_ty: Type,
    ref_kind: RefKind,
    is_mut: bool,
}

struct OutputParam {
    name: Ident,
    ty: Type,
}

struct StageConfig {
    original_fn: ItemFn,
    stage_name: Ident,
    handle_name: Ident,
    is_lazy: bool,
    cache: CacheStrategy,
    inputs: Vec<InputParam>,
    outputs: Vec<OutputParam>,
    state_type: TokenStream2,
    generics: syn::Generics,
}

// --- attribute parsing ------------------------------------------------------

struct Outputs(Punctuated<Output, Token![,]>);

impl Parse for Outputs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Punctuated::parse_terminated(input).map(Self)
    }
}

struct Output {
    name: Ident,
    ty: Type,
}

impl Parse for Output {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        let _: Token![:] = input.parse()?;
        let ty = input.parse()?;
        Ok(Output { name, ty })
    }
}

enum StageArg {
    Flag(Ident),
    Outputs(Outputs),
    State(Type),
}

impl Parse for StageArg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        if ident == "out" {
            let content;
            syn::parenthesized!(content in input);
            return Ok(StageArg::Outputs(content.parse()?));
        }
        if ident == "state" {
            let content;
            syn::parenthesized!(content in input);
            return Ok(StageArg::State(content.parse()?));
        }
        Ok(StageArg::Flag(ident))
    }
}

struct StageArgs {
    args: Punctuated<StageArg, Token![,]>,
}

impl Parse for StageArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(StageArgs {
            args: Punctuated::parse_terminated(input)?,
        })
    }
}

// --- configuration ----------------------------------------------------------

impl StageConfig {
    fn from_args(input_fn: &ItemFn, args: &StageArgs) -> syn::Result<Self> {
        let stage_name = input_fn.sig.ident.clone();
        let handle_name = format_ident!("{}Handle", stage_name);

        let mut is_lazy = false;
        let mut cache = CacheStrategy::None;
        let mut explicit_outputs = Vec::new();
        let mut state_type = quote!(());

        for arg in &args.args {
            match arg {
                StageArg::Flag(ident) => match ident.to_string().as_str() {
                    "lazy" => is_lazy = true,
                    "cache_last" => cache = CacheStrategy::Last,
                    "cache_all" => cache = CacheStrategy::All,
                    other => {
                        return Err(syn::Error::new(
                            ident.span(),
                            format!("unrecognized stage attribute `{other}`"),
                        ));
                    }
                },
                StageArg::Outputs(outputs) => {
                    for output in &outputs.0 {
                        explicit_outputs.push(OutputParam {
                            name: output.name.clone(),
                            ty: output.ty.clone(),
                        });
                    }
                }
                StageArg::State(ty) => state_type = quote!(#ty),
            }
        }

        let inputs = Self::extract_inputs(&input_fn.sig.inputs)?;

        // Only type generics are supported: lifetimes cannot be `'static`, and
        // const generics have no `TypeId`.
        let generics = input_fn.sig.generics.clone();
        for param in &generics.params {
            match param {
                syn::GenericParam::Type(_) => {}
                syn::GenericParam::Lifetime(lifetime) => {
                    return Err(syn::Error::new_spanned(
                        lifetime,
                        "lifetime parameters are not supported on stages",
                    ));
                }
                syn::GenericParam::Const(constant) => {
                    return Err(syn::Error::new_spanned(
                        constant,
                        "const generic parameters are not supported on stages",
                    ));
                }
            }
        }

        let outputs = if explicit_outputs.is_empty() {
            vec![OutputParam {
                name: format_ident!("out"),
                ty: return_type(&input_fn.sig.output),
            }]
        } else {
            explicit_outputs
        };

        Ok(StageConfig {
            original_fn: input_fn.clone(),
            stage_name,
            handle_name,
            is_lazy,
            cache,
            inputs,
            outputs,
            state_type,
            generics,
        })
    }

    fn extract_inputs(inputs: &Punctuated<FnArg, Token![,]>) -> syn::Result<Vec<InputParam>> {
        let mut result = Vec::new();
        for arg in inputs {
            let FnArg::Typed(pat_type) = arg else {
                return Err(syn::Error::new_spanned(arg, "`self` is not supported"));
            };
            let Pat::Ident(pat_ident) = &*pat_type.pat else {
                return Err(syn::Error::new_spanned(
                    &pat_type.pat,
                    "only simple identifiers are supported as inputs",
                ));
            };
            let ident = pat_ident.ident.clone();
            let raw = ident.to_string();
            let clean = raw.strip_prefix('_').unwrap_or(&raw).to_string();
            let is_mut = pat_ident.mutability.is_some();

            let (ref_kind, true_ty) = match &*pat_type.ty {
                Type::Reference(reference) => {
                    let kind = if reference.mutability.is_some() {
                        RefKind::BorrowedMut
                    } else {
                        RefKind::Borrowed
                    };
                    (kind, (*reference.elem).clone())
                }
                other => (RefKind::Owned, other.clone()),
            };

            result.push(InputParam {
                ident,
                clean,
                true_ty,
                ref_kind,
                is_mut,
            });
        }
        Ok(result)
    }
}

fn return_type(output: &ReturnType) -> Type {
    match output {
        ReturnType::Type(_, ty) => (**ty).clone(),
        ReturnType::Default => syn::parse_quote!(()),
    }
}

// --- code generation --------------------------------------------------------

fn input_ops_expr(cache: CacheStrategy, ty: &Type) -> TokenStream2 {
    match cache {
        CacheStrategy::None => quote!(directed::ValueOps::opaque::<#ty>()),
        CacheStrategy::Last => quote!(directed::ValueOps::eq::<#ty>()),
        CacheStrategy::All => quote!(directed::ValueOps::eq_hash::<#ty>()),
    }
}

fn output_ops_expr(ty: &Type) -> TokenStream2 {
    // Outputs are only ever shared by `Arc` clone, so they need no
    // comparison capabilities regardless of the node's cache policy.
    quote!(directed::ValueOps::opaque::<#ty>())
}

fn ref_kind_expr(kind: RefKind) -> TokenStream2 {
    match kind {
        RefKind::Owned => quote!(directed::RefKind::Owned),
        RefKind::Borrowed => quote!(directed::RefKind::Borrowed),
        RefKind::BorrowedMut => quote!(directed::RefKind::BorrowedMut),
    }
}

fn expand(config: StageConfig) -> TokenStream2 {
    let StageConfig {
        original_fn,
        stage_name,
        handle_name,
        is_lazy,
        cache,
        inputs,
        outputs,
        state_type,
        generics,
    } = config;

    let type_params: Vec<Ident> = generics
        .type_params()
        .map(|param| param.ident.clone())
        .collect();
    let is_generic = !type_params.is_empty();

    // Every type parameter must satisfy the requirements the runtime places on
    // port types, plus whatever the cache policy needs.
    let mut generics = generics;
    if is_generic {
        let mut bounds: Vec<syn::TypeParamBound> = vec![
            syn::parse_quote!(Clone),
            syn::parse_quote!(Send),
            syn::parse_quote!(Sync),
            syn::parse_quote!('static),
        ];
        if cache != CacheStrategy::None {
            bounds.push(syn::parse_quote!(PartialEq));
        }
        if cache == CacheStrategy::All {
            bounds.push(syn::parse_quote!(Hash));
        }
        let where_clause = generics.make_where_clause();
        for ident in &type_params {
            let bounds = bounds.clone();
            where_clause
                .predicates
                .push(syn::parse_quote!(#ident: #(#bounds)+*));
        }
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let vis = &original_fn.vis;
    let attrs = &original_fn.attrs;
    let original_args = &original_fn.sig.inputs;
    let ret_ty = &original_fn.sig.output;
    let body = &original_fn.block;
    let asyncness = &original_fn.sig.asyncness;
    let is_async = asyncness.is_some();
    let maybe_await = if is_async { quote!(.await) } else { quote!() };

    let eval = if is_lazy {
        quote!(directed::EvalStrategy::Lazy)
    } else {
        quote!(directed::EvalStrategy::Urgent)
    };
    let cache_tokens = match cache {
        CacheStrategy::None => quote!(directed::CachePolicy::None),
        CacheStrategy::Last => quote!(directed::CachePolicy::Last),
        CacheStrategy::All => quote!(directed::CachePolicy::All),
    };

    // Signature ports.
    let input_ports = inputs.iter().map(|input| {
        let name = syn::LitStr::new(&input.clean, Span::call_site());
        let kind = ref_kind_expr(input.ref_kind);
        let ty = &input.true_ty;
        let ops = input_ops_expr(cache, ty);
        quote! {
            directed::InputPort { name: #name, ref_kind: #kind, ops: #ops }
        }
    });
    let output_ports = outputs.iter().map(|output| {
        let name = syn::LitStr::new(&output.name.to_string(), Span::call_site());
        let ty = &output.ty;
        let ops = output_ops_expr(ty);
        quote! {
            directed::OutputPort { name: #name, ops: #ops }
        }
    });

    // Typed handle accessor methods.
    let input_methods = inputs.iter().enumerate().map(|(index, input)| {
        let method = format_ident!("{}", input.clean);
        let ty = &input.true_ty;
        let index_u16 = index as u16;
        quote! {
            pub fn #method(&self) -> directed::PortIn<#ty> {
                directed::PortIn::new(
                    self.id,
                    #index_u16,
                    &<#stage_name #ty_generics as directed::Stage>::signature().inputs[#index],
                )
            }
        }
    });
    let output_methods = outputs.iter().enumerate().map(|(index, output)| {
        let method = &output.name;
        let ty = &output.ty;
        let index_u16 = index as u16;
        quote! {
            pub fn #method(&self) -> directed::PortOut<#ty> {
                directed::PortOut::new(
                    self.id,
                    #index_u16,
                    &<#stage_name #ty_generics as directed::Stage>::signature().outputs[#index],
                )
            }
        }
    });

    // Input extraction from the `Io` buffer.
    let extraction = inputs.iter().enumerate().map(|(index, input)| {
        let ident = &input.ident;
        let true_ty = &input.true_ty;
        let mutability = if input.is_mut { quote!(mut) } else { quote!() };
        match input.ref_kind {
            RefKind::Borrowed => quote! {
                let #mutability #ident = io.get::<#true_ty>(#index)?;
            },
            RefKind::Owned => quote! {
                let #mutability #ident = io.take_cloned::<#true_ty>(#index)?;
            },
            // A `&mut` input is copied into the node and mutated locally.
            RefKind::BorrowedMut => {
                let owned = format_ident!("__{}_owned", ident);
                quote! {
                    let mut #owned = io.take_cloned::<#true_ty>(#index)?;
                    let #ident = &mut #owned;
                }
            }
        }
    });
    let arg_idents = inputs.iter().map(|input| &input.ident);

    // Output handling.
    let output_handling = if outputs.len() == 1 {
        quote! { io.set(0usize, __result); }
    } else {
        let binders: Vec<Ident> = (0..outputs.len())
            .map(|index| format_ident!("__out{}", index))
            .collect();
        let sets = binders
            .iter()
            .enumerate()
            .map(|(index, binder)| quote! { io.set(#index, #binder); });
        quote! {
            let (#(#binders),*) = __result;
            #(#sets)*
        }
    };

    let (body_fn, body_call) = if is_generic {
        let turbofish = quote!(::<#(#type_params),*>);
        (
            quote! {
                #[allow(non_snake_case, clippy::too_many_arguments)]
                #asyncness fn __body #impl_generics (state: &mut #state_type, #original_args) #ret_ty #where_clause {
                    #body
                }
            },
            quote!(__body #turbofish (state, #(#arg_idents),*) #maybe_await),
        )
    } else {
        (
            quote! {
                #[allow(non_snake_case, clippy::too_many_arguments)]
                #asyncness fn __body(state: &mut #state_type, #original_args) #ret_ty {
                    #body
                }
            },
            quote!(__body(state, #(#arg_idents),*) #maybe_await),
        )
    };

    let signature_fn = if is_generic {
        quote! {
            fn signature() -> &'static directed::Signature {
                // Key on the concrete stage type: distinct generic stages with
                // the same type arguments must not share a signature.
                directed::intern_signature(
                    vec![std::any::TypeId::of::<#stage_name #ty_generics>()],
                    || directed::Signature {
                        stage: stringify!(#stage_name),
                        inputs: vec![ #(#input_ports),* ],
                        outputs: vec![ #(#output_ports),* ],
                    },
                )
            }
        }
    } else {
        quote! {
            fn signature() -> &'static directed::Signature {
                static __SIGNATURE: std::sync::OnceLock<directed::Signature> =
                    std::sync::OnceLock::new();
                __SIGNATURE.get_or_init(|| directed::Signature {
                    stage: stringify!(#stage_name),
                    inputs: vec![ #(#input_ports),* ],
                    outputs: vec![ #(#output_ports),* ],
                })
            }
        }
    };

    let handle_fn = if is_generic {
        quote! {
            #handle_name { id, _marker: std::marker::PhantomData }
        }
    } else {
        quote! {
            #handle_name { id }
        }
    };

    let (stage_def, handle_def, handle_copy) = if is_generic {
        let phantom = quote!(( #( #type_params, )* ));
        (
            quote! {
                #[allow(non_camel_case_types)]
                #vis struct #stage_name #impl_generics (
                    std::marker::PhantomData<fn() -> #phantom>
                ) #where_clause;
            },
            quote! {
                #[allow(non_camel_case_types)]
                #vis struct #handle_name #impl_generics #where_clause {
                    id: directed::NodeId,
                    _marker: std::marker::PhantomData<fn() -> #phantom>,
                }
            },
            quote! {
                impl #impl_generics Clone for #handle_name #ty_generics #where_clause {
                    fn clone(&self) -> Self {
                        *self
                    }
                }
                impl #impl_generics Copy for #handle_name #ty_generics #where_clause {}
            },
        )
    } else {
        (
            quote! {
                #[allow(non_camel_case_types)]
                #[derive(Clone, Copy)]
                #vis struct #stage_name;
            },
            quote! {
                #[allow(non_camel_case_types)]
                #[derive(Clone, Copy)]
                #vis struct #handle_name {
                    id: directed::NodeId,
                }
            },
            quote! {},
        )
    };

    quote! {
        #(#attrs)*
        #stage_def
        #handle_def
        #handle_copy

        impl #impl_generics #handle_name #ty_generics #where_clause {
            #(#input_methods)*
            #(#output_methods)*
        }

        impl #impl_generics directed::StageHandle for #handle_name #ty_generics #where_clause {
            fn id(&self) -> directed::NodeId {
                self.id
            }
        }

        impl #impl_generics directed::Stage for #stage_name #ty_generics #where_clause {
            type State = #state_type;
            type Handle = #handle_name #ty_generics;

            const EVAL: directed::EvalStrategy = #eval;
            const CACHE: directed::CachePolicy = #cache_tokens;

            #signature_fn

            fn handle(id: directed::NodeId) -> Self::Handle {
                #handle_fn
            }

            fn call<'a>(
                state: &'a mut #state_type,
                io: &'a mut directed::Io,
            ) -> impl std::future::Future<Output = Result<(), directed::CallError>> + Send + 'a {
                #body_fn
                async move {
                    #(#extraction)*
                    let __result = #body_call;
                    #output_handling
                    Ok(())
                }
            }
        }
    }
}
