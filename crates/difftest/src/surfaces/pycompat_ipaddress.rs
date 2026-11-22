//! The `pycompat.ipaddress` difftest surface (P1's oracle `port/difftest/oracle/pycompat.py`).
//!
//! `ipaddress.IPv4Address(s).packed` and `ipaddress.IPv6Address(s)` (its groups, and whether it
//! is in `fc00::/7` and `fe80::/10`) against `pycompat::ipaddress`. Seeds are the privacy
//! cases (`tests/test_memory_privacy.py`), zones included (`fe80::1%en0`, empty and doubled
//! `%`), and the edges of Python's parser; generated text mixes digits, hex, `.`, `:`, `%`.

use anthology_pycompat::ipaddress;
use anthology_pycompat::json::Value;

use super::pycompat_str::{field, gen_cases, int, obj, ok, pool_string, python, raised, s};
use crate::case::{Case, Rng, Surface};
use crate::observe::Observed;
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.ipaddress";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatIpaddress))
}

struct PycompatIpaddress;

const V4: &[&str] = &[
    "0.0.0.0",
    "1.2.3.4",
    "1.2.3.4.5",
    "10.1.2.3",
    "100.64.3.4",
    "127.0.0.1",
    "127.8.9.10",
    "169.254.10.10",
    "172.16.0.9",
    "172.31.255.1",
    "172.32.0.1",
    "192.0.2.1",
    "192.168.1.20",
    "198.51.100.7",
    "203.0.113.200",
    "255.255.255.255",
    "8.8.4.4",
    "999.1.2.3",
    "01.2.3.4",
    "1.2.3.04",
    "1.2.3",
    "",
    "1.2.3.4/8",
    "١.2.3.4",
    " 1.2.3.4",
    "1.2.3.4 ",
    "1..2.3",
    "256.0.0.0",
    "1.2.3.+4",
    "0x1.2.3.4",
    "1.2.3.4%en0",
    "0000.1.1.1",
];

const V6: &[&str] = &[
    "2001:db8::1",
    "fd12:3456:789a::1",
    "fe80::1%en0",
    "fe80::1",
    "fe80::1%",
    "fe80::1%a%b",
    "::",
    "::1",
    "1::",
    ":",
    ":::",
    "1:2:3:4:5:6:7:8",
    "1:2:3:4:5:6:7:8:9",
    "1:2:3:4:5:6:7",
    "1::2::3",
    "::ffff:1.2.3.4",
    "::ffff:1.2.3.04",
    "1:2:3:4:5:6:1.2.3.4",
    "1:2:3:4:5:6:7:1.2.3.4",
    "12345::",
    "fe80::",
    "febf::1",
    "fec0::1",
    "fc00::",
    "fbff::1",
    "FE80::A%eth0.1",
    "1:",
    ":1",
    "1::2:",
    "::1:",
    "%en0",
    "::1/128",
    "g::1",
    "1:2:3:4:5:6:7::",
    "::2:3:4:5:6:7:8",
    "1::3:4:5:6:7:8",
    "0:0:0:0:0:0:0:0",
    "::1.2.3.4",
    "fe80::1%%",
    "1:2:3:4:5:6:7:8:9:10:11:12",
    "a:b:c:d:e:f:a:b:c:1.2.3.4",
];

const POOL: &[char] = &[
    '0', '1', '2', '9', 'a', 'f', 'F', 'g', ':', ':', ':', '.', '.', '%', 'e', 'n', '/', '١',
];

fn case(op: &str, v: &str) -> Value {
    obj(vec![("op", s(op)), ("s", s(v))])
}

impl Surface for PycompatIpaddress {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = Vec::new();
        for (i, v) in V4.iter().enumerate() {
            out.push(Case::new(format!("v4:{}", i + 1), case("ipaddress.v4", v)));
        }
        for (i, v) in V6.iter().enumerate() {
            out.push(Case::new(format!("v6:{}", i + 1), case("ipaddress.v6", v)));
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        gen_cases(n, || {
            let v = pool_string(rng, POOL, 20);
            let op = if rng.chance(0.4) {
                "ipaddress.v4"
            } else {
                "ipaddress.v6"
            };
            case(op, &v)
        })
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        python(c, oracle)
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let v = field(&c.data, "s");
        match field(&c.data, "op") {
            "ipaddress.v4" => match ipaddress::parse_v4(v) {
                Some(o) => ok(&Value::Array(o.iter().map(|&b| int(b.into())).collect())),
                None => raised("AddressValueError"),
            },
            "ipaddress.v6" => match ipaddress::parse_v6(v) {
                Some(g) => ok(&obj(vec![
                    (
                        "groups",
                        Value::Array(g.iter().map(|&x| int(x.into())).collect()),
                    ),
                    (
                        "unique_local",
                        Value::Bool(ipaddress::is_unique_local_v6(&g)),
                    ),
                    ("link_local", Value::Bool(ipaddress::is_link_local_v6(&g))),
                ])),
                None => raised("AddressValueError"),
            },
            op => panic!("pycompat.ipaddress: unknown op {op:?}"),
        }
    }
}
