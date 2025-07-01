use proc_macro2::Span;
use syn::{
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    spanned::Spanned,
};

// Configuration structs
#[derive(Clone)]
pub(crate) struct StageConfig {
    pub(crate) original_fn: syn::ItemFn,
    pub(crate) stage_name: syn::Ident,
    pub(crate) is_lazy: (bool, Span),
    pub(crate) is_async: (bool, Span),
    pub(crate) cache_strategy: (CacheStrategy, Span),
    pub(crate) outputs: OutputParams,
    pub(crate) inputs: Vec<InputParam>,
    pub(crate) states: Vec<(syn::Ident, syn::Type, Span)>,
}

#[derive(Clone)]
pub(crate) enum RefType {
    Owned,
    Borrowed,
    BorrowedMut,
}

#[derive(Clone)]
pub(crate) enum OutputParams {
    Explicit(Vec<OutputParam>),
    Implicit(syn::Type, Span),
}

#[derive(Clone)]
pub(crate) struct OutputParam {
    pub(crate) name: syn::Ident,
    pub(crate) ty: syn::Type,
    pub(crate) span: Span,
}

#[derive(Clone)]
pub(crate) struct InputParam {
    pub(crate) ty: syn::Type,
    pub(crate) ref_type: RefType,
    pub(crate) clean_name: syn::Ident,
    pub(crate) span: Span,
}

impl InputParam {
    pub(crate) fn unwrapped_type(&self) -> &syn::Type {
        match &self.ty {
            syn::Type::Reference(type_reference) => &*type_reference.elem,
            ty => &ty,
        }
    }
}

#[derive(Clone)]
pub(crate) struct TupleParam {
    pub(crate) idx: syn::Index,
    pub(crate) ty: syn::Type,
    pub(crate) span: Span,
}

pub(crate) trait Param {
    fn param(&self) -> (syn::Member, &syn::Type, &Span);
}

impl Param for OutputParam {
    fn param(&self) -> (syn::Member, &syn::Type, &Span) {
        (self.name.clone().into(), &self.ty, &self.span)
    }
}

impl Param for &OutputParam {
    fn param(&self) -> (syn::Member, &syn::Type, &Span) {
        (self.name.clone().into(), &self.ty, &self.span)
    }
}

impl Param for InputParam {
    fn param(&self) -> (syn::Member, &syn::Type, &Span) {
        (self.clean_name.clone().into(), &self.ty, &self.span)
    }
}

impl Param for &InputParam {
    fn param(&self) -> (syn::Member, &syn::Type, &Span) {
        (self.clean_name.clone().into(), &self.ty, &self.span)
    }
}

impl Param for TupleParam {
    fn param(&self) -> (syn::Member, &syn::Type, &Span) {
        (self.idx.clone().into(), &self.ty, &self.span)
    }
}

impl Param for &TupleParam {
    fn param(&self) -> (syn::Member, &syn::Type, &Span) {
        (self.idx.clone().into(), &self.ty, &self.span)
    }
}

#[derive(Clone)]
pub(crate) struct ParamList(pub(crate) Punctuated<ParamLike, syn::Token![,]>);

impl Parse for ParamList {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Punctuated::parse_terminated(input).map(Self)
    }
}

#[derive(Clone)]
pub(crate) struct ParamLike {
    pub(crate) name: syn::Ident,
    pub(crate) ty: syn::Type,
    pub(crate) span: Span,
}

impl Parse for ParamLike {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: syn::Ident = input.parse()?;
        let _colon_token: syn::Token![:] = input.parse()?;
        let ty: syn::Type = input.parse()?;
        Ok(ParamLike {
            name,
            ty,
            span: Span::call_site(),
        })
    }
}

pub(crate) enum StageArg {
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

pub(crate) struct StageArgs {
    pub(crate) args: Punctuated<StageArg, syn::Token![,]>,
}

impl Parse for StageArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(StageArgs {
            args: Punctuated::parse_terminated(input)?,
        })
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub(crate) enum CacheStrategy {
    None,
    Last,
    All,
}

impl StageConfig {
    pub(crate) fn from_args(input_fn: &syn::ItemFn, meta_args: &StageArgs) -> syn::Result<Self> {
        let stage_name = input_fn.sig.ident.clone();

        let is_async = (
            input_fn.sig.asyncness.is_some(),
            input_fn.sig.asyncness.span(),
        );
        let mut is_lazy = (false, Span::call_site());
        let mut cache_strategy = (CacheStrategy::None, Span::call_site());
        let mut outputs = match &input_fn.sig.output {
            syn::ReturnType::Default => OutputParams::Implicit(
                syn::Type::Tuple(syn::TypeTuple {
                    paren_token: syn::token::Paren::default(),
                    elems: Punctuated::new(),
                }),
                Span::call_site(),
            ),
            syn::ReturnType::Type(_, ty) => OutputParams::Implicit(*ty.clone(), ty.span()),
        };
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
                    if matches!(outputs, OutputParams::Implicit(..)) {
                        outputs = OutputParams::Explicit(Vec::new());
                    }
                    for output in &param_list.0 {
                        match &mut outputs {
                            OutputParams::Explicit(output_params) => {
                                output_params.push(OutputParam {
                                    name: output.name.clone(),
                                    ty: output.ty.clone(),
                                    span: output.span,
                                })
                            }
                            _ => unreachable!(),
                        }
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

        Ok(StageConfig {
            original_fn: input_fn.clone(),
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
        inputs: &syn::punctuated::Punctuated<syn::FnArg, syn::Token![,]>,
    ) -> syn::Result<Vec<InputParam>> {
        let mut result = Vec::new();

        for arg in inputs.iter() {
            if let syn::FnArg::Typed(pat_type) = arg {
                if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                    let arg_name = &pat_ident.ident;
                    let arg_type = &pat_type.ty;
                    let arg_name_str = arg_name.to_string();

                    let is_unused = arg_name_str.starts_with('_');
                    let clean_name = if is_unused {
                        syn::Ident::new(&arg_name_str[1..], arg_name.span())
                    } else {
                        arg_name.clone()
                    };

                    let ref_type = match &**arg_type {
                        syn::Type::Reference(type_reference)
                            if type_reference.mutability.is_some() =>
                        {
                            RefType::BorrowedMut
                        }
                        syn::Type::Reference(_) => RefType::Borrowed,
                        _ => RefType::Owned,
                    };

                    result.push(InputParam {
                        ty: *arg_type.clone(),
                        ref_type,
                        clean_name,
                        span: arg.span(),
                    });
                }
            }
        }

        Ok(result)
    }
}
