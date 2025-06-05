/// Simple macro to simulate a function that can return multiple names outputs
#[macro_export]
macro_rules! output {
    ($($name:ident $(: $val:expr)?),*) => {
        StageOutputType {
        $(
            $name: output!(@internal $name $(, $val)?),
        )*
        }
    };
    (@internal $name:ident, $val:expr) => { $val };
    (@internal $name:ident) => { $name };
}
