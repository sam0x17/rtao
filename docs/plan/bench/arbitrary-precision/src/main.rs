use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum WeightsArg { Map(std::collections::BTreeMap<String, f64>), List(Vec<(u16, f64)>) }

#[derive(Debug, Deserialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum AmountIn { Num(f64), Str(String) }

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Inner { rate_tolerance: f64 }
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Outer { #[serde(flatten)] inner: Inner }

fn r<T: std::fmt::Debug, E: std::fmt::Display>(x: Result<T, E>) -> String { match x { Ok(_) => "ok".into(), Err(e) => format!("ERR {e}") } }

fn main() {
    println!("feature arbitrary_precision on: {}", cfg!(feature = "ap"));
    println!("untagged map from_str 0.5:      {}", r(serde_json::from_str::<WeightsArg>(r#"{"1": 0.5}"#)));
    println!("untagged list from_str 0.5:     {}", r(serde_json::from_str::<WeightsArg>(r#"[[1, 0.5]]"#)));
    println!("untagged list from_value 0.5:   {}", r(serde_json::from_value::<WeightsArg>(serde_json::json!([[1, 0.5]]))));
    println!("untagged num from_str 1.5:      {}", r(serde_json::from_str::<AmountIn>("1.5")));
    println!("untagged num from_str 7:        {}", r(serde_json::from_str::<AmountIn>("7")));
    println!("flatten from_str 0.05:          {}", r(serde_json::from_str::<Outer>(r#"{"rate_tolerance": 0.05}"#)));
    let x: serde_json::Value = serde_json::from_str("1e-05").unwrap();
    let y: serde_json::Value = serde_json::from_str("1e-5").unwrap();
    println!("Value(1e-05) == Value(1e-5):    {}", x == y);
}
