use proc_macro::TokenStream;
use proc_macro_error::proc_macro_error;
use quote::quote;
use syn::{
    FnArg, ItemFn, Pat, ReturnType, Token, Type,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
};

/// A macro that wraps a function with the standardized interface:
/// fn fn_name(&mut DataMap, &DataMap) -> Result<DataMap>
///
/// Example usage:
///
/// ```
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
struct StageConfig {
    original_fn: ItemFn,
    stage_name: syn::Ident,
    is_lazy: bool,
    cache_strategy: CacheStrategy,
    outputs: Vec<(String, Type)>,
    inputs: Vec<InputParam>,
    state_type: proc_macro2::TokenStream
}

enum RefType {
    Owned,
    Borrowed,
    BorrowedMut,
}

impl RefType {
    fn quoted(&self) -> proc_macro2::TokenStream {
        match self {
            RefType::Owned => quote! { directed::RefType::Owned },
            RefType::Borrowed => quote! { directed::RefType::Borrowed },
            RefType::BorrowedMut => quote! { directed::RefType::BorrowedMut },
        }
    }
}

struct InputParam {
    name: syn::Ident,
    type_: Type,
    ref_type: RefType,
    clean_name: String,
}

#[derive(Clone)]
struct Outputs(Punctuated<Output, Token![,]>);

impl Parse for Outputs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Punctuated::parse_terminated(input).map(Self)
    }
}

#[derive(Clone)]
struct Output {
    name: syn::Ident,
    ty: Type,
}

impl Parse for Output {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        let _colon_token: Token![:] = input.parse()?;
        let ty = input.parse()?;
        Ok(Output { name, ty })
    }
}

enum StageArg {
    Flag(syn::Ident),
    Output(Outputs),
    State(syn::Type)
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

#[derive(PartialEq)]
enum CacheStrategy {
    None,
    Last,
    All,
}

impl StageConfig {
    fn from_args(input_fn: &ItemFn, meta_args: &StageArgs) -> syn::Result<Self> {
        let stage_name = input_fn.sig.ident.clone();

        let mut is_lazy = false;
        let mut cache_strategy = CacheStrategy::None;
        let mut outputs = Vec::new();
        let mut state_type = quote!(());

        // Process stage attribute arguments
        for arg in meta_args.args.iter() {
            match arg {
                StageArg::Flag(ident) => match ident.to_string().as_str() {
                    "lazy" => is_lazy = true,
                    "cache_last" => cache_strategy = CacheStrategy::Last,
                    "cache_all" => cache_strategy = CacheStrategy::All,
                    unknown => {
                        return Err(syn::Error::new(
                            ident.span(),
                            format!("Unrecognized attribute: {}", unknown),
                        ));
                    }
                },
                StageArg::Output(output_defs) => {
                    for output in &output_defs.0 {
                        outputs.push((output.name.to_string(), output.ty.clone()));
                    }
                },
                StageArg::State(ty) => {
                    state_type = quote!(#ty);
                }
            }
        }

        // Process function arguments to create input definitions
        let inputs = Self::extract_input_params(&input_fn.sig.inputs)?;

        // If no outputs specified, process return type
        if outputs.is_empty() {
            outputs = Self::extract_outputs_from_return_type(&input_fn.sig.output)?;
        }

        Ok(StageConfig {
            original_fn: input_fn.clone(),
            stage_name,
            is_lazy,
            cache_strategy,
            outputs,
            inputs,
            state_type
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
                    });
                }
            }
        }

        Ok(result)
    }

    fn extract_outputs_from_return_type(
        return_type: &ReturnType,
    ) -> syn::Result<Vec<(String, Type)>> {
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
                Ok(vec![("_".to_string(), (**ty).clone())])
            }
            ReturnType::Default => {
                // Return type is (), use default name
                Ok(vec![(
                    "_".to_string(),
                    Type::Tuple(syn::TypeTuple {
                        paren_token: syn::token::Paren::default(),
                        elems: Punctuated::new(),
                    }),
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
        
        quote! {
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
fn generate_output_registrations(outputs: &[(String, Type)]) -> Vec<proc_macro2::TokenStream> {
    outputs
        .iter()
        .map(|(name, ty)| {
            quote! {
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
fn generate_extraction_code_move(inputs: &[InputParam]) -> Vec<proc_macro2::TokenStream> {
    inputs.iter().map(|input| {
        let arg_name = &input.name;
        let arg_type = true_type(&input.type_);
        let clean_arg_name = &input.clean_name;
        let reeval_name = quote::format_ident!("{}_reevaluation_rule", clean_arg_name);
        
        quote! {
            // Non-transparent functions never clone, always move
            let (#arg_name, #reeval_name): (std::sync::Arc<#arg_type>, directed::ReevaluationRule) = if let Some((input, reeval_rule)) = inputs.remove(&directed::DataLabel::new(#clean_arg_name)) {
                let dc = std::sync::Arc::downcast::<#arg_type>(input);
                match dc {
                    Ok(val) => (val, reeval_rule),
                    Err(e) => return Err(directed::InjectionError::InputTypeMismatchDetails{ name: #clean_arg_name, expected: stringify!(#arg_type)})
                }
            } else {
                return Err(directed::InjectionError::InputNotFound(#clean_arg_name.into()));
            };
        }
    }).collect()
}

fn generate_extraction_code_cache_last(inputs: &[InputParam]) -> Vec<proc_macro2::TokenStream> {
    inputs.iter().map(|input| {
        let arg_name = &input.name;
        let arg_type = true_type(&input.type_);
        let clean_arg_name = &input.clean_name;
        let reeval_name = quote::format_ident!("{}_reevaluation_rule", clean_arg_name);
        
        quote! {
            let (#arg_name, #reeval_name): (std::sync::Arc<#arg_type>, directed::ReevaluationRule) = if let Some((input, reeval_rule)) = inputs.get(&directed::DataLabel::new(#clean_arg_name)) {
                match std::sync::Arc::downcast::<#arg_type>(input.clone()) {
                    Ok(val) => (val, *reeval_rule),
                    Err(_) => return Err(directed::InjectionError::InputTypeMismatchDetails{ name: #clean_arg_name, expected: stringify!(#arg_type)})
                }
            } else {
                return Err(directed::InjectionError::InputNotFound(#clean_arg_name.into()));
            };
        }
    }).collect()
}

/// This generates the code that uses the output of a parent node to set the
/// input of a child node. This function is for moves only.
fn inject_opaque_out(inputs: &[InputParam]) -> Vec<proc_macro2::TokenStream> {
    let mut code = inputs.iter().map(|input| {
        let clean_arg_name = &input.clean_name;
        let arg_type = &input.type_;
        
        quote! {
            #clean_arg_name => {
                let input_changed = node.input_changed();
                let output_val = parent.outputs_mut()
                    .remove(&output)
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?;
                let output_val = std::sync::Arc::downcast::<#arg_type>(output_val)
                    .map_err(|_| directed::InjectionError::OutputTypeMismatch(output.clone()))?;
                node.inputs_mut().insert(input, (output_val, directed::ReevaluationRule::Move));
                Ok(())
            }
        }
    }).collect::<Vec<_>>();

    // Add the default case
    code.push(quote! {
        name => Err(directed::InjectionError::InputNotFound(name.into()))
    });

    code
}

/// This generates the code that uses the output of a parent node to set the
/// input of a child node. An equality comparison will be done between new
/// output and the previous input, and a flag is raised if they don't match.
fn inject_transparent_out_to_owned_in(inputs: &[InputParam]) -> Vec<proc_macro2::TokenStream> {
    let mut code = inputs.iter().map(|input| {
        let clean_arg_name = &input.clean_name;
        let arg_type = true_type(&input.type_);
        
        quote! {
            #clean_arg_name => {
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
        }
    }).collect::<Vec<_>>();

    // Add the default case
    code.push(quote! {
        name => Err(directed::InjectionError::InputNotFound(name.into()))
    });

    code
}

fn inject_transparent_out_to_opaque_ref_in(inputs: &[InputParam]) -> Vec<proc_macro2::TokenStream> {
    let mut code = inputs.iter().map(|input| {
        let clean_arg_name = &input.clean_name;
        let arg_type = true_type(&input.type_);
        
        quote! {
            #clean_arg_name => {
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
        }
    }).collect::<Vec<_>>();

    // Add the default case
    code.push(quote! {
        name => Err(directed::InjectionError::InputNotFound(name.into()))
    });

    code
}

/// Functions that return a NodeOutput are used as-is, where as functions
/// that return anything else are wrapped in a simple MultOutput (simple
/// in that it contains only 1 output named '_')
fn generate_output_handling(config: &StageConfig) -> proc_macro2::TokenStream {
    let arg_names = config.inputs.iter().map(|input| &input.name);
    if config.is_multi_output() {
        quote! {
            Ok(Self::get_fn()(state, #(#arg_names),*))
        }
    } else {
        quote! {
            Ok(directed::NodeOutput::new_simple(Self::get_fn()(state, #(#arg_names),*)))
        }
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
                output.push(quote!{
                    let #arg_name = match #reeval_name {
                        directed::ReevaluationRule::Move => {
                            // Parent is opaque, use Arc::into_inner
                            match std::sync::Arc::into_inner(#arg_name) {
                                Some(arg) => arg,
                                None => {return Err(directed::InjectionError::TooManyReferences(stringify!(#arg_name)))}
                            }
                        },
                        directed::ReevaluationRule::CacheLast => {
                            // Parent is transparent, clone the value
                            (*#arg_name).clone()
                        },
                        directed::ReevaluationRule::CacheAll => panic!("CacheAll is not yet implemented"),
                    };
                });
            }
            RefType::Borrowed => {
                output.push(quote! {
                    // TODO: if node is transparent (config.cache_strategy != None), error with a graceful message (rather than letting clone fail)
                    let #arg_name = #arg_name.as_ref();
                });
            }
            RefType::BorrowedMut => panic!("Mutable refs are not yet supported"),
        }
    }
    output
}

/// The core trait that defines a stage - the culimnation of this macro
fn generate_stage_impl(config: StageConfig) -> proc_macro2::TokenStream {
    let original_fn = &config.original_fn;
    let stage_name = &config.stage_name;
    let state_type = &config.state_type;
    let fn_attrs = &original_fn.attrs;
    let fn_vis = &original_fn.vis;
    let original_args = &original_fn.sig.inputs;
    let fn_return_type = &original_fn.sig.output;
    let original_body = &original_fn.block;

    // Generate code sections
    let input_registrations = generate_input_registrations(&config.inputs);
    let output_registrations = generate_output_registrations(&config.outputs);
    let extraction_code = match config.cache_strategy {
        CacheStrategy::None => generate_extraction_code_move(&config.inputs),
        CacheStrategy::Last => generate_extraction_code_cache_last(&config.inputs),
        CacheStrategy::All => todo!(), // TODO: Handle CacheAll extraction
    };
    let inject_opaque_out_code = inject_opaque_out(&config.inputs);
    let inject_transparent_out_to_owned_in_code =
        inject_transparent_out_to_owned_in(&config.inputs);
    let inject_transparent_out_to_opaque_ref_in_code =
        inject_transparent_out_to_opaque_ref_in(&config.inputs);
    let prepare_input_types_code = prepare_input_types(&config);
    let output_handling = generate_output_handling(&config);

    // Determine evaluation strategy and reevaluation rule
    let eval_strategy = if config.is_lazy {
        quote! { directed::EvalStrategy::Lazy }
    } else {
        quote! { directed::EvalStrategy::Urgent }
    };

    let reevaluation_rule = match config.cache_strategy {
        CacheStrategy::None => quote! { directed::ReevaluationRule::Move },
        CacheStrategy::Last => quote! { directed::ReevaluationRule::CacheLast },
        CacheStrategy::All => todo!(), //quote! { directed::ReevaluationRule::CacheAll },
    };

    // The coup de grace
    quote! {
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

        impl directed::Stage for #stage_name {
            type State = #state_type;
            type BaseFn = fn(state: &mut #state_type, #original_args) #fn_return_type;

            fn inputs(&self) -> &std::collections::HashMap<directed::DataLabel, (std::any::TypeId, directed::RefType)> {
                &self.inputs
            }

            fn outputs(&self) -> &std::collections::HashMap<directed::DataLabel, std::any::TypeId> {
                &self.outputs
            }

            fn evaluate(&self, state: &mut Self::State, inputs: &mut std::collections::HashMap<directed::DataLabel, (std::sync::Arc<dyn std::any::Any + Send + Sync>, directed::ReevaluationRule)>) -> Result<directed::NodeOutput, directed::InjectionError> {
                #(#extraction_code)*
                #(#prepare_input_types_code)*
                #output_handling
            }

            fn eval_strategy(&self) -> directed::EvalStrategy {
                #eval_strategy
            }

            fn reeval_rule(&self) -> directed::ReevaluationRule {
                #reevaluation_rule
            }

            // TODO: This can be simplified to be a bit less unruly
            fn inject_input(&self, node: &mut directed::Node<Self>, parent: &mut Box<dyn directed::AnyNode>, output: directed::DataLabel, input: directed::DataLabel) -> Result<(), directed::InjectionError> {
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

            fn name(&self) -> &str {
                stringify!(#stage_name)
            }

            fn get_fn() -> Self::BaseFn {
                #(#fn_attrs)*
                fn original_fn(state: &mut #state_type, #original_args) #fn_return_type #original_body
                return original_fn;
            }
        }
    }
}
