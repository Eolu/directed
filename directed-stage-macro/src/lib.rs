use proc_macro::TokenStream;
use proc_macro_error::proc_macro_error;
use proc_macro2::Span;
use quote::quote_spanned;
use syn::{
    FnArg, ItemFn, Pat, ReturnType, Token, Type,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    spanned::Spanned,
};

// TODO: `inject_input` implementation is unruly and violates DRY in silly ways. It could be simplified a lot.

/// A macro that wraps a function with the standardized interface:
/// fn fn_name(&mut DataMap, &DataMap) -> Result<DataMap>
///
/// Example usage:
///
/// ```ignore
/// use crate::*;
///
/// #[stage(lazy, transparent)]
/// fn add_numbers(a: i32, b: i32) -> i32 {
///     a + b
/// }
/// ```
///
/// Multiple outputs are also supported with this syntax:
/// #[stage(out(arg1_name: String, arg2_name: Vec<u8>))]
/// fn output_things() -> directed::NodeOutput {
///    let some_string = String::from("Hello Graph!");
///    let some_vec = vec![1, 2, 3, 4, 5];
///
///    // This builds an output type
///    directed::output!{
///        arg1_name: some_string,
///        arg2_name: some_vec
///    }
/// }
#[proc_macro_attribute]
#[proc_macro_error]
pub fn stage(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    let meta_args = parse_macro_input!(attr as StageArgs);
    generate_stage_impl(StageConfig::from_args(&input_fn, &meta_args).unwrap()).into()
}

// Configuration structs
#[derive(Clone)]
struct StageConfig {
    original_fn: ItemFn,
    original_fn_renamed: syn::Ident,
    stage_name: syn::Ident,
    is_lazy: (bool, Span),
    is_async: (bool, Span),
    cache_strategy: (CacheStrategy, Span),
    // TODO: Why is this a String? Use ident directly
    outputs: Vec<(String, Type, Span)>,
    inputs: Vec<InputParam>,
    states: Vec<(syn::Ident, Type, Span)>,
}

#[derive(Clone)]
enum RefType {
    Owned,
    Borrowed,
    BorrowedMut,
}

impl RefType {
    fn quoted(&self) -> proc_macro2::TokenStream {
        match self {
            RefType::Owned => quote_spanned! {Span::call_site()=> directed::RefType::Owned },
            RefType::Borrowed => quote_spanned! {Span::call_site()=> directed::RefType::Borrowed },
            RefType::BorrowedMut => {
                quote_spanned! {Span::call_site()=> directed::RefType::BorrowedMut }
            }
        }
    }
}

#[derive(Clone)]
struct InputParam {
    name: syn::Ident,
    type_: Type,
    ref_type: RefType,
    clean_name: String,
    span: Span,
}

#[derive(Clone)]
struct ParamList(Punctuated<ParamLike, Token![,]>);

impl Parse for ParamList {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Punctuated::parse_terminated(input).map(Self)
    }
}

#[derive(Clone)]
struct ParamLike {
    name: syn::Ident,
    ty: Type,
    span: Span,
}

impl Parse for ParamLike {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: syn::Ident = input.parse()?;
        let _colon_token: Token![:] = input.parse()?;
        let ty: syn::Type = input.parse()?;
        Ok(ParamLike {
            name,
            ty,
            span: Span::call_site(),
        })
    }
}

enum StageArg {
    Flag(syn::Ident),
    Output(ParamList),
    State(ParamList),
}

impl Parse for StageArg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let lookahead = input.lookahead1();

        if lookahead.peek(syn::Ident) {
            let ident: syn::Ident = input.parse()?;

            if ident == "out" {
                let content;
                let _paren_token = syn::parenthesized!(content in input);
                return Ok(StageArg::Output(content.parse()?));
            } else if ident == "state" {
                let content;
                let _paren_token = syn::parenthesized!(content in input);
                return Ok(StageArg::State(content.parse()?));
            } else {
                return Ok(StageArg::Flag(ident));
            }
        }

        Err(lookahead.error())
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

#[derive(PartialEq, Eq, Clone, Copy)]
enum CacheStrategy {
    None,
    Last,
    All,
}

impl StageConfig {
    fn from_args(input_fn: &ItemFn, meta_args: &StageArgs) -> syn::Result<Self> {
        let stage_name = input_fn.sig.ident.clone();

        let is_async = (
            input_fn.sig.asyncness.is_some(),
            input_fn.sig.asyncness.span(),
        );
        let mut is_lazy = (false, Span::call_site());
        let mut cache_strategy = (CacheStrategy::None, Span::call_site());
        let mut outputs = Vec::new();
        let mut states = Vec::new();

        // Process stage attribute arguments
        for arg in meta_args.args.iter() {
            match arg {
                StageArg::Flag(ident) => match ident.to_string().as_str() {
                    "lazy" => is_lazy = (true, ident.span()),
                    "cache_last" => cache_strategy = (CacheStrategy::Last, ident.span()),
                    "cache_all" => cache_strategy = (CacheStrategy::All, ident.span()),
                    unknown => {
                        return Err(syn::Error::new(
                            ident.span(),
                            format!("Unrecognized attribute: {}", unknown),
                        ));
                    }
                },
                StageArg::Output(param_list) => {
                    for output in &param_list.0 {
                        outputs.push((output.name.to_string(), output.ty.clone(), output.span));
                    }
                }
                StageArg::State(param_list) => {
                    for state in &param_list.0 {
                        states.push((state.name.clone(), state.ty.clone(), state.span));
                    }
                }
            }
        }

        // Process function arguments to create input definitions
        let inputs = Self::extract_input_params(&input_fn.sig.inputs)?;

        // If no outputs specified, process return type
        if outputs.is_empty() {
            outputs = Self::extract_outputs_from_return_type(&input_fn.sig.output)?;
        }

        let original_fn_renamed = quote::format_ident!("original_fn_{}", stage_name);

        Ok(StageConfig {
            original_fn: input_fn.clone(),
            original_fn_renamed,
            stage_name,
            is_lazy,
            is_async,
            cache_strategy,
            outputs,
            inputs,
            states,
        })
    }

    fn extract_input_params(
        inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>,
    ) -> syn::Result<Vec<InputParam>> {
        let mut result = Vec::new();

        for arg in inputs.iter() {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(pat_ident) = &*pat_type.pat {
                    let arg_name = &pat_ident.ident;
                    let arg_type = &pat_type.ty;
                    let arg_name_str = arg_name.to_string();

                    let is_unused = arg_name_str.starts_with('_');
                    let clean_name = if is_unused {
                        arg_name_str[1..].to_string()
                    } else {
                        arg_name_str.clone()
                    };

                    let ref_type = match &**arg_type {
                        Type::Reference(type_reference) if type_reference.mutability.is_some() => {
                            RefType::BorrowedMut
                        }
                        Type::Reference(_) => RefType::Borrowed,
                        _ => RefType::Owned,
                    };

                    result.push(InputParam {
                        name: arg_name.clone(),
                        type_: *arg_type.clone(),
                        ref_type,
                        clean_name,
                        span: arg.span(),
                    });
                }
            }
        }

        Ok(result)
    }

    fn extract_outputs_from_return_type(
        return_type: &ReturnType,
    ) -> syn::Result<Vec<(String, Type, Span)>> {
        match return_type {
            ReturnType::Type(_, ty) => {
                // Check if the return type is NodeOutput
                if let Type::Path(type_path) = &**ty {
                    if let Some(segment) = type_path.path.segments.last() {
                        // TODO: This is a hack, just properly check if any out attributes exist
                        if segment.ident == "NodeOutput" {
                            // NodeOutput will be handled elsewhere
                            return Ok(Vec::new());
                        }
                    }
                }

                // Single output with default name
                Ok(vec![("_".to_string(), (**ty).clone(), ty.span())])
            }
            ReturnType::Default => {
                // Return type is (), use default name
                Ok(vec![(
                    "_".to_string(),
                    Type::Tuple(syn::TypeTuple {
                        paren_token: syn::token::Paren::default(),
                        elems: Punctuated::new(),
                    }),
                    Span::mixed_site(),
                )])
            }
        }
    }

    fn is_multi_output(&self) -> bool {
        if let ReturnType::Type(_, ty) = &self.original_fn.sig.output {
            if let Type::Path(type_path) = &**ty {
                if let Some(segment) = type_path.path.segments.last() {
                    // TODO: This is a hack, just check if any out attributes exist
                    return segment.ident == "NodeOutput";
                }
            }
        }
        false
    }
}

/// This associates the names of function parameters with the TypeId of their type.
///
/// Used to build and validate I/O-based connections
fn generate_input_registrations(inputs: &[InputParam]) -> Vec<proc_macro2::TokenStream> {
    inputs.iter().map(|input| {
        let arg_name = &input.clean_name;
        let arg_type = &input.type_;
        let ref_type = input.ref_type.quoted();
        let span = input.span.clone();
        quote_spanned! {span=>
            inputs.insert(directed::DataLabel::new_with_type_name(#arg_name, stringify!(#arg_type)), (std::any::TypeId::of::<#arg_type>(), #ref_type));
        }
    }).collect()
}

/// This associates the names of function outputs with the TypeId of their type.
/// When a function returns a NodeOutput type, this will associate meaningful
/// names to each output. When a function returns any other type, this will
/// simply associate that one type with the name '_'.
///
/// Used to build and validate I/O-based connections
fn generate_output_registrations(
    outputs: &[(String, Type, Span)],
) -> Vec<proc_macro2::TokenStream> {
    outputs
        .iter()
        .map(|(name, ty, _span)| {
            quote_spanned! {Span::call_site()=>
                // TODO: Fix the fact it can't find outputs here!
                outputs.insert(directed::DataLabel::new_with_type_name(#name, stringify!(#ty)), std::any::TypeId::of::<#ty>());
            }
        })
        .collect()
}

/// Get a type, squash the &
fn true_type(ty: &syn::Type) -> &syn::Type {
    if let syn::Type::Reference(ty) = ty {
        &*ty.elem
    } else {
        ty
    }
}

/// This code is used by the wrapper function - it downcasts type-erased
/// function parameters so that the user-facing function can be called with
/// concrete types.
fn generate_extraction_code(
    inputs: &[InputParam],
    cache_strategy: (CacheStrategy, Span),
) -> Vec<proc_macro2::TokenStream> {
    inputs.iter().map(|input| {
        let arg_name = &input.name;
        let arg_type = true_type(&input.type_);
        let clean_arg_name = &input.clean_name;
        let reeval_name = quote::format_ident!("{}_reevaluation_rule", clean_arg_name);
        let input_span = input.span.clone();

        match cache_strategy {
            (CacheStrategy::None, _span) => quote_spanned! {input_span=>
                // Non-transparent functions never clone, always move
                let #reeval_name: directed::ReevaluationRule = inputs.get(&directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type)))
                    .map(|(_, reeval_rule)| *reeval_rule).ok_or_else(|| directed::InjectionError::InputNotFound(directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type))))?;
                let #arg_name: std::sync::Arc<#arg_type> = if #reeval_name == directed::ReevaluationRule::Move {
                    if let Some((input, _)) = inputs.remove(&directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type))) {
                        let dc = std::sync::Arc::downcast::<#arg_type>(input);
                        match dc {
                            Ok(val) => val,
                            #[allow(unused_variables)]
                            Err(e) => return Err(directed::InjectionError::InputTypeMismatchDetails{ name: #clean_arg_name, expected: stringify!(#arg_type)})
                        }
                    } else {
                        return Err(directed::InjectionError::InputNotFound(directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type))));
                    }
                } else {
                    if let Some((input, _)) = inputs.get(&directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type))) {
                        match std::sync::Arc::downcast::<#arg_type>(input.clone()) {
                            Ok(val) => val,
                            Err(_) => return Err(directed::InjectionError::InputTypeMismatchDetails{ name: #clean_arg_name, expected: stringify!(#arg_type)})
                        }
                    } else {
                        return Err(directed::InjectionError::InputNotFound(directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type))));
                    }
                };
            },
            // TODO: Check if "All" needs something different
            (CacheStrategy::Last, _span) | (CacheStrategy::All, _span) => quote_spanned! {input_span=>
                let (#arg_name, #reeval_name): (std::sync::Arc<#arg_type>, directed::ReevaluationRule) = if let Some((input, reeval_rule)) = inputs.get(&directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type))) {
                    match std::sync::Arc::downcast::<#arg_type>(input.clone()) {
                        Ok(val) => (val, *reeval_rule),
                        Err(_) => return Err(directed::InjectionError::InputTypeMismatchDetails{ name: #clean_arg_name, expected: stringify!(#arg_type)})
                    }
                } else {
                    return Err(directed::InjectionError::InputNotFound(directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type))));
                };
            },
        }
    }).collect()
}

/// This generates the code that uses the output of a parent node to set the
/// input of a child node.
fn input_injection(inputs: &[InputParam]) -> proc_macro2::TokenStream {
    let mut inject_opaque_out_code = Vec::new();
    let mut inject_transparent_out_to_owned_in_code = Vec::new();
    let mut inject_transparent_out_to_opaque_ref_in_code = Vec::new();

    // Build match arms from inputs
    for input in inputs.iter() {
        let clean_arg_name = &input.clean_name;
        let arg_type = true_type(&input.type_);
        let span = input.span.clone();

        inject_opaque_out_code.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                #[allow(unused_variables)]
                let input_changed = node.input_changed();
                let output_val = parent.outputs_mut()
                    .remove(&output)
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?;
                let output_val = std::sync::Arc::downcast::<#arg_type>(output_val)
                    .map_err(|_| directed::InjectionError::OutputTypeMismatch(output.clone()))?;
                node.inputs_mut().insert(input, (output_val, directed::ReevaluationRule::Move));
                Ok(())
            }
        });
        inject_transparent_out_to_owned_in_code.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                #[allow(unused_variables)]
                let input_changed = node.input_changed();
                let output_val = parent.outputs_mut()
                    .get(&output)
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?
                    .clone(); // Clone the Arc
                let output_val = std::sync::Arc::downcast::<#arg_type>(output_val)
                    .map_err(|_| directed::InjectionError::OutputTypeMismatch(output.clone()))?;
                match node.inputs_mut().get(&input) {
                    Some((input_val, _)) => {
                        let input_val = input_val
                            .downcast_ref::<#arg_type>()
                            .ok_or_else(|| directed::InjectionError::InputTypeMismatch(input.clone()))?;
                        if !input_changed && output_val.as_ref() != input_val {
                            node.set_input_changed(true);
                        }
                    },
                    None => {
                        node.set_input_changed(true);
                    }
                }

                node.inputs_mut().insert(input, (output_val, directed::ReevaluationRule::CacheLast));
                Ok(())
            }
        });
        inject_transparent_out_to_opaque_ref_in_code.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                #[allow(unused_variables)]
                let input_changed = node.input_changed();
                let output_val_arc = parent.outputs_mut()
                    .get(&output)
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?;
                let output_val_ref = std::sync::Arc::downcast::<#arg_type>(output_val_arc.clone())
                    .map_err(|_| directed::InjectionError::InputTypeMismatch(input.clone()))?;
                match node.inputs_mut().get(&input) {
                    Some((input_val, _)) => {
                        let input_val = input_val
                            .downcast_ref::<#arg_type>()
                            .ok_or_else(|| directed::InjectionError::InputTypeMismatch(input.clone()))?;
                        if !input_changed && input_val != &*output_val_ref {
                            node.set_input_changed(true);
                        }
                    },
                    None => {
                        node.set_input_changed(true);
                    }
                }

                node.inputs_mut().insert(input, (output_val_ref, directed::ReevaluationRule::CacheLast));
                Ok(())
            }
        });
    }

    // Add the default case
    let default_case = quote_spanned! {Span::call_site()=>
        Some(name) => Err(directed::InjectionError::InputNotFound(name.into())),
        None => Ok(()) // This means there's a connection with no data associated
    };
    inject_opaque_out_code.push(default_case.clone());
    inject_transparent_out_to_owned_in_code.push(default_case.clone());
    inject_transparent_out_to_opaque_ref_in_code.push(default_case);

    quote_spanned! {Span::call_site()=>
        fn inject_opaque_out(node: &mut dyn directed::AnyNode, parent: &mut Box<dyn directed::AnyNode>, output: directed::DataLabel, input: directed::DataLabel) -> Result<(), directed::InjectionError> {
            match input.inner() {
                #(#inject_opaque_out_code)*
            }
        }
        fn inject_transparent_out_to_owned_in(node: &mut dyn directed::AnyNode, parent: &mut Box<dyn directed::AnyNode>, output: directed::DataLabel, input: directed::DataLabel) -> Result<(), directed::InjectionError> {
            match input.inner() {
                #(#inject_transparent_out_to_owned_in_code)*
            }
        }
        fn inject_transparent_out_to_opaque_ref_in(node: &mut dyn directed::AnyNode, parent: &mut Box<dyn directed::AnyNode>, output: directed::DataLabel, input: directed::DataLabel) -> Result<(), directed::InjectionError> {
            match input.inner() {
                #(#inject_transparent_out_to_opaque_ref_in_code)*
            }
        }

        if parent.reeval_rule() == directed::ReevaluationRule::Move {
            if node.reeval_rule() == directed::ReevaluationRule::Move && node.input_reftype(&input) != Some(directed::RefType::Owned) {
                inject_transparent_out_to_opaque_ref_in(node, parent, output, input)
            } else {
                inject_opaque_out(node, parent, output, input)
            }
        } else {
            inject_transparent_out_to_owned_in(node, parent, output, input)
        }
    }
}

/// Functions that return a NodeOutput are used as-is, where as functions
/// that return anything else are wrapped in a simple MultOutput (simple
/// in that it contains only 1 output named '_')
fn generate_output_handling(
    config: &StageConfig,
    cache_strategy: (CacheStrategy, Span),
) -> proc_macro2::TokenStream {
    // TODO: Right now cache_last and cache_all are handled more differently than necessary
    let original_fn_renamed = &config.original_fn_renamed;
    let arg_names = config
        .inputs
        .iter()
        .map(|input| &input.name)
        .collect::<Vec<_>>();
    let clean_names = config
        .inputs
        .iter()
        .map(|input| &input.clean_name)
        .collect::<Vec<_>>();
    let arg_types = config
        .inputs
        .iter()
        .map(|input| &input.type_)
        .collect::<Vec<_>>();
    let downcast_ref_calls = clean_names.iter().zip(arg_types.iter()).zip(arg_names.iter()).map(|((clean_name, arg_type), arg_name)| {
        let name_span = arg_name.span();
        quote_spanned!{name_span=>
            if let Some(cached_in) = cached.inputs.get(&directed::DataLabel::new_with_type_name(#clean_name, stringify!(#arg_type))) {
                if let Some(dc) = in_val.0.downcast_ref::<#arg_type>() {
                    if dc.downcast_eq(&**cached_in) {
                        return true;
                    }
                }
            }
        }
    }).collect::<Vec<_>>();
    let state_as_inputs = match config.states.len(){
        0 => vec![],
        1 => {
            vec![quote_spanned!{config.states[0].2=> state}]
        },
        _ => config.states.iter()
                .enumerate()
                .map(|(i, state)| {
                    let idx = syn::Index::from(i);
                    quote_spanned!{state.2=> &mut state.#idx}
                })
                .collect::<Vec<_>>()
    };
    // TODO: Get all output types annotated
    let first_output_type = config
        .outputs
        .iter()
        .map(|(_, t, _)| t)
        .cloned()
        .next()
        .unwrap_or(Type::Tuple(syn::TypeTuple {
            paren_token: syn::token::Paren::default(),
            elems: Punctuated::new(),
        }));
    let fn_call = match (config.is_multi_output(), config.is_async.0) {
        (true, true) => {
            quote_spanned! {Span::call_site()=> #original_fn_renamed(#(#state_as_inputs),* #(#arg_names),*).await }
        }
        (true, false) => {
            quote_spanned! {Span::call_site()=> #original_fn_renamed(#(#state_as_inputs),* #(#arg_names),*) }
        }
        (false, true) => {
            quote_spanned! {Span::call_site()=> directed::NodeOutput::new_simple(#original_fn_renamed(#(#state_as_inputs),* #(#arg_names),*).await) }
        }
        (false, false) => {
            quote_spanned! {Span::call_site()=> directed::NodeOutput::new_simple(#original_fn_renamed(#(#state_as_inputs),* #(#arg_names),*)) }
        }
    };

    if cache_strategy.0 == CacheStrategy::All {
        quote_spanned! {cache_strategy.1=>
            // Use a hasher
            let hash: u64 = {
                #[allow(unused_imports)]
                use std::hash::Hash;
                #[allow(unused_imports)]
                use std::hash::Hasher;
                #[allow(unused_mut)]
                let mut hasher = std::hash::DefaultHasher::new();
                #(#arg_names.hash(&mut hasher);)*
                hasher.finish()
            };

            // Check equality
            #[allow(unused_variables)]
            let cached = cache.get(&hash).and_then(|cached| {
                #[allow(unused_imports)]
                use directed::DowncastEq;
                cached.iter().find(|cached| {
                    inputs.iter().all(|(in_name, in_val)| {
                        #(#downcast_ref_calls)*
                        false
                    })
                })
            });

            if let Some(cached) = cached {
                // Just use cached values
                if cached.outputs.len() == 1 && cached.outputs.get(&"_".into()).is_some() {
                    // TODO: Don't panic
                    Ok(NodeOutput::dyn_new_simple(cached.outputs.get(&"_".into()).unwrap().clone()))
                } else {
                    let mut result = NodeOutput::new();
                    for (out_name, out_val) in cached.outputs.iter() {
                        result = result.add(&out_name.name.to_owned().expect("Expected a non-blank data label"), out_val.clone());
                    }
                    Ok(result)
                }
            } else {
                // Call and store result in cache
                let result = #fn_call;
                let cache_entry = {
                    #[allow(unused_mut)]
                    let mut cached = directed::Cached {
                        inputs: std::collections::HashMap::new(),
                        outputs: std::collections::HashMap::new(),
                    };
                    for (in_name, in_val) in inputs.iter() {
                        cached.inputs.insert(in_name.clone(), in_val.0.clone());
                    }
                    match &result {
                        NodeOutput::Standard(val) => {
                            cached.outputs.insert(directed::DataLabel::new_with_type_name("_", stringify!(#first_output_type)), val.clone());
                        },
                        NodeOutput::Named(vals) => {
                            for (key, val) in vals {
                                cached.outputs.insert(key.clone(), val.clone());
                            }
                        },
                    }
                    cached
                };
                if let None = cache.get(&hash) {
                    cache.insert(hash, Vec::new());
                }
                if let Some(vec) = cache.get_mut(&hash) {
                    vec.push(cache_entry);
                }
                Ok(result)
            }
        }
    } else {
        // No advanced caching, just run it
        quote_spanned!(cache_strategy.1=>Ok(#fn_call))
    }
}

fn prepare_input_types(config: &StageConfig) -> Vec<proc_macro2::TokenStream> {
    let args = config
        .inputs
        .iter()
        .map(|input| (&input.name, &input.clean_name, &input.ref_type));
    let mut output = Vec::new();
    for (arg_name, clean_name, ref_type) in args {
        let reeval_name = quote::format_ident!("{}_reevaluation_rule", clean_name);
        match ref_type {
            RefType::Owned => {
                output.push(quote_spanned!{arg_name.span()=>
                    let #arg_name = match #reeval_name {
                        directed::ReevaluationRule::Move => {
                            // Parent is opaque, use Arc::into_inner
                            match std::sync::Arc::into_inner(#arg_name) {
                                Some(arg) => arg,
                                None => {return Err(directed::InjectionError::TooManyReferences(stringify!(#arg_name)))}
                            }
                        },
                        directed::ReevaluationRule::CacheLast | directed::ReevaluationRule::CacheAll => {
                            // Parent is transparent, clone the value
                            (*#arg_name).clone()
                        },
                    };
                });
            }
            RefType::Borrowed => {
                output.push(quote_spanned! {arg_name.span()=>
                    // TODO: if node is transparent (config.cache_strategy != None), error with a graceful message (rather than letting clone fail)
                    let #arg_name = #arg_name.as_ref();
                });
            }
            RefType::BorrowedMut => panic!("Mutable refs are not yet supported"),
        }
    }
    output
}

/// The core trait that defines a stage - the culmination of this macro
fn generate_stage_impl(config: StageConfig) -> proc_macro2::TokenStream {
    let original_fn = &config.original_fn;
    let stage_name = &config.stage_name;
    let states = &config.states;
    let original_fn_renamed = &config.original_fn_renamed;
    let fn_attrs = &original_fn.attrs;
    let fn_vis = &original_fn.vis;
    let original_args = &original_fn.sig.inputs;
    let fn_return_type = &original_fn.sig.output;
    let original_body = &original_fn.block;
    let async_status = &original_fn.sig.asyncness;

    // Generate code sections
    let input_registrations = generate_input_registrations(&config.inputs);
    let output_registrations = generate_output_registrations(&config.outputs);
    let extraction_code = generate_extraction_code(&config.inputs, config.cache_strategy);
    let injection_code = input_injection(&config.inputs);
    let prepare_input_types_code = prepare_input_types(&config);
    let output_handling = generate_output_handling(&config, config.cache_strategy);

    // Determine evaluation strategy and reevaluation rule
    let eval_strategy = if config.is_lazy.0 {
        quote_spanned! {config.is_lazy.1=> directed::EvalStrategy::Lazy }
    } else {
        quote_spanned! {config.is_lazy.1=> directed::EvalStrategy::Urgent }
    };

    let reevaluation_rule = match &config.cache_strategy {
        (CacheStrategy::None, span) => quote_spanned! {*span=> directed::ReevaluationRule::Move },
        (CacheStrategy::Last, span) => {
            quote_spanned! {*span=> directed::ReevaluationRule::CacheLast }
        }
        (CacheStrategy::All, span) => {
            quote_spanned! {*span=> directed::ReevaluationRule::CacheAll }
        }
    };

    // Generate function inputs
    let state_as_input = states.iter().map(|(name,ty,span)| quote_spanned! {*span=>#name: &mut #ty});
    let state_as_type = match states.len() {
        0 => quote::quote!{()},
        1 => {
            let (_,ty,span) = &states[0];
            quote_spanned! {*span=>#ty}
        },
        _ => {
            let types = states.iter().map(|(_,ty,span)| quote_spanned! {*span=>#ty});
            quote::quote!{(#(#types),*,)}
        }
    };

    // Redefine the original function
    let new_fn_definition = quote::quote! {
        #(#fn_attrs)*
        #async_status fn #original_fn_renamed(#(#state_as_input),* #original_args) #fn_return_type #original_body
    };

    // Slap together eval logic
    let eval_logic = quote_spanned! {Span::call_site()=>
        #(#extraction_code)*
        #(#prepare_input_types_code)*
        #output_handling
    };
    let sync_eval_logic = if async_status.is_some() {
        quote::quote!(panic!("Attempted to call async code synchronously"))
    } else {
        quote::quote!(#eval_logic)
    };
    let async_eval_logic = if async_status.is_some() {
        quote::quote!(#eval_logic)
    } else {
        quote::quote!(#eval_logic)
    };

    // TODO: Try and move away from type erasure by doing more concrete magic
    // let input_struct_name = quote::format_ident!{"{stage_name}InputCache"};
    // let output_struct_name = quote::format_ident!{"{stage_name}OutputCache"};

    // let input_struct_fields = proc_macro2::TokenStream::from_iter(config.inputs.iter().map(|input| {
    //     let input_ident = syn::Ident::new(&input.clean_name, input.span);
    //     let mut input_ty = &input.type_;
    //     quote_spanned!{input.span=>
    //         #input_ident: Option<#input_ty>,
    //     }
    // }));
    // let input_struct = quote_spanned!{Span::call_site()=> 
    //     #fn_vis struct #input_struct_name {
    //         #input_struct_fields
    //     }
    // };

    // let output_struct = if config.outputs.len() == 1 && config.outputs[0].0 == "_" {
    //     let (_, ty, span) = &config.outputs[0];
    //     quote_spanned!{*span=> 
    //         #fn_vis struct #output_struct_name(#ty);
    //     }
    // } else {
    //     let output_struct_fields = proc_macro2::TokenStream::from_iter(config.outputs.iter().map(|(name, ty, span)| {
    //         let output_ident = syn::Ident::new(name, *span);
    //         let output_ty = &ty;
    //         quote_spanned!{*span=>
    //             #output_ident: Option<#output_ty>,
    //         }
    //     }));
    //     quote_spanned!{Span::call_site()=> 
    //         #fn_vis struct #output_struct_name {
    //             #output_struct_fields
    //         }
    //     }
    // };


    // The coup de grace
    quote_spanned! {Span::call_site()=>
        // Create a struct implementing the Stage trait
        #[derive(Clone)]
        #fn_vis struct #stage_name {
            inputs: std::collections::HashMap<directed::DataLabel, (std::any::TypeId, directed::RefType)>,
            outputs: std::collections::HashMap<directed::DataLabel, std::any::TypeId>,
        }

        impl #stage_name {
            pub fn new() -> Self {
                let mut inputs = std::collections::HashMap::new();
                let mut outputs = std::collections::HashMap::new();
                #(#input_registrations)*
                #(#output_registrations)*
                Self { inputs, outputs }
            }
        }

        // Recreate the original function
        #[allow(non_snake_case)]
        #new_fn_definition

        #[cfg_attr(feature = "tokio", async_trait::async_trait)]
        impl directed::Stage for #stage_name {
            type State = #state_as_type;

            fn inputs(&self) -> &std::collections::HashMap<directed::DataLabel, (std::any::TypeId, directed::RefType)> {
                &self.inputs
            }

            fn outputs(&self) -> &std::collections::HashMap<directed::DataLabel, std::any::TypeId> {
                &self.outputs
            }

            fn evaluate(
                &self,
                state: &mut Self::State,
                inputs: &mut std::collections::HashMap<directed::DataLabel, (std::sync::Arc<dyn std::any::Any + Send + Sync>, directed::ReevaluationRule)>,
                cache: &mut std::collections::HashMap<u64, Vec<directed::Cached>>
            ) -> Result<directed::NodeOutput, directed::InjectionError> {
                #sync_eval_logic
            }

            #[cfg(feature = "tokio")]
            async fn evaluate_async(
                &self,
                state: &mut Self::State,
                inputs: &mut std::collections::HashMap<directed::DataLabel, (std::sync::Arc<dyn std::any::Any + Send + Sync>, directed::ReevaluationRule)>,
                cache: &mut std::collections::HashMap<u64, Vec<directed::Cached>>
            ) -> Result<directed::NodeOutput, directed::InjectionError> {
                #async_eval_logic
            }

            fn eval_strategy(&self) -> directed::EvalStrategy {
                #eval_strategy
            }

            fn reeval_rule(&self) -> directed::ReevaluationRule {
                #reevaluation_rule
            }

            fn inject_input(&self, node: &mut directed::Node<Self>, parent: &mut Box<dyn directed::AnyNode>, output: directed::DataLabel, input: directed::DataLabel) -> Result<(), directed::InjectionError> {
                #injection_code
            }

            fn name(&self) -> &str {
                stringify!(#stage_name)
            }
        }
    }
}
