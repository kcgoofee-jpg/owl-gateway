//! Offline IP → ASN lookup (anti-abuse; used by the daily scan job, not on the request hot path).
//!
//! Data: iptoasn.com `ip2asn-combined.tsv.gz`, TSV columns
//! `range_start  range_end  AS_number  country_code  AS_description`, IPv4 and IPv6 mixed,
//! `AS_number == 0` means "not routed" (skipped at load time).
//!
//! Memory layout: ranges are struct-of-arrays (start / end / meta index) per address family;
//! `(asn, country, name)` triples are interned into a small meta table, names stored once.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read};
use std::net::IpAddr;
use std::path::Path;

use flate2::read::MultiGzDecoder;

/// Datacenter / cloud / VPN keywords, matched case-insensitively against the AS name after
/// `-` / `_` are turned into spaces. Curated to avoid consumer ISPs (China Telecom / Unicom /
/// Mobile, Comcast, …): no generic words like "net", "telecom", "broadband". Avoid short
/// fragments that hit country names (e.g. "colo" would match "Colombia").
const HOSTING_KEYWORDS: &[&str] = &[
    "hosting",
    "host ",
    "hoster",
    "cloud",
    "server",
    "datacenter",
    "data center",
    "datacentre",
    "data centre",
    "vps",
    "colocation",
    "dedicated",
    "amazon",
    "aws",
    "google cloud",
    "microsoft",
    "azure",
    "digitalocean",
    "linode",
    "akamai",
    "ovh",
    "hetzner",
    "vultr",
    "choopa",
    "contabo",
    "leaseweb",
    "m247",
    "oracle",
    "alibaba",
    "aliyun",
    "tencent cloud",
    "huawei cloud",
    "cloudflare",
    "fastly",
    "zenlayer",
    "bitsflow",
    "gcore",
    "g core",
    "datacamp",
    "packethub",
    "mullvad",
    "nordvpn",
    "expressvpn",
    "proton",
    "private internet access",
    "ipxo",
    "scaleway",
    "upcloud",
    "kamatera",
    "hostinger",
    "ionos",
    "psychz",
    "quadranet",
    "hurricane electric",
    "multacom",
    "frantech",
    "buyvm",
    "racknerd",
    "colocrossing",
    "dmit",
    "ucloud",
];

/// Heuristic: does this AS name look like a datacenter / cloud / VPN network?
pub fn is_hosting_name(name: &str) -> bool {
    let mut norm: String = name
        .chars()
        .map(|c| match c {
            '-' | '_' | ',' | '.' => ' ',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    norm.push(' ');
    // word-ish match for short tokens that would otherwise hit inside other words
    let words: Vec<&str> = norm.split_whitespace().collect();
    if words.iter().any(|w| matches!(*w, "aws" | "ovh" | "vps")) {
        return true;
    }
    HOSTING_KEYWORDS
        .iter()
        .filter(|k| !matches!(**k, "aws" | "ovh" | "vps"))
        .any(|k| norm.contains(k))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsnInfo {
    pub asn: u32,
    pub country: String,
    pub name: String,
    pub hosting: bool,
}

#[derive(Debug, Clone, Copy)]
struct Meta {
    asn: u32,
    country: [u8; 2],
    name: u32,
}

#[derive(Debug, Default)]
struct Table<T> {
    starts: Vec<T>,
    ends: Vec<T>,
    meta: Vec<u32>,
}

impl<T: Ord + Copy> Table<T> {
    fn build(mut rows: Vec<(T, T, u32)>) -> Self {
        rows.sort_unstable_by_key(|r| r.0);
        let mut t = Table {
            starts: Vec::with_capacity(rows.len()),
            ends: Vec::with_capacity(rows.len()),
            meta: Vec::with_capacity(rows.len()),
        };
        for (s, e, m) in rows {
            t.starts.push(s);
            t.ends.push(e);
            t.meta.push(m);
        }
        t
    }

    fn find(&self, ip: T) -> Option<u32> {
        let i = self.starts.partition_point(|&s| s <= ip).checked_sub(1)?;
        (ip <= self.ends[i]).then(|| self.meta[i])
    }
}

#[derive(Debug, Default)]
pub struct AsnDb {
    v4: Table<u32>,
    v6: Table<u128>,
    metas: Vec<Meta>,
    names: Vec<Box<str>>,
    hosting: Vec<bool>,
}

impl AsnDb {
    /// Load a `.tsv.gz` (detected by gzip magic) or plain `.tsv` file.
    pub fn load(path: &Path) -> io::Result<AsnDb> {
        let mut f = BufReader::with_capacity(1 << 16, std::fs::File::open(path)?);
        let gz = f.fill_buf()?.starts_with(&[0x1f, 0x8b]);
        if gz {
            Self::from_reader(BufReader::with_capacity(1 << 16, MultiGzDecoder::new(f)))
        } else {
            Self::from_reader(f)
        }
    }

    /// Parse TSV rows. Malformed lines are skipped (count is logged).
    pub fn from_reader<R: Read>(r: BufReader<R>) -> io::Result<AsnDb> {
        let mut db = AsnDb::default();
        let mut name_idx: HashMap<Box<str>, u32> = HashMap::new();
        let mut meta_idx: HashMap<(u32, [u8; 2], u32), u32> = HashMap::new();
        let mut v4 = Vec::new();
        let mut v6 = Vec::new();
        let mut bad = 0usize;
        for line in r.lines() {
            let line = line?;
            let mut cols = line.splitn(5, '\t');
            let (Some(a), Some(b), Some(asn), Some(cc), Some(name)) = (
                cols.next(),
                cols.next(),
                cols.next(),
                cols.next(),
                cols.next(),
            ) else {
                if !line.trim().is_empty() {
                    bad += 1;
                }
                continue;
            };
            let Ok(asn) = asn.trim().parse::<u32>() else {
                bad += 1;
                continue;
            };
            if asn == 0 {
                continue;
            }
            let (Ok(a), Ok(b)) = (a.parse::<IpAddr>(), b.parse::<IpAddr>()) else {
                bad += 1;
                continue;
            };
            let country = match cc.as_bytes() {
                [x, y] => [x.to_ascii_uppercase(), y.to_ascii_uppercase()],
                _ => [0, 0],
            };
            let name = name.trim();
            let n = match name_idx.get(name) {
                Some(&n) => n,
                None => {
                    let n = db.names.len() as u32;
                    db.names.push(name.into());
                    db.hosting.push(is_hosting_name(name));
                    name_idx.insert(name.into(), n);
                    n
                }
            };
            let m = *meta_idx.entry((asn, country, n)).or_insert_with(|| {
                db.metas.push(Meta {
                    asn,
                    country,
                    name: n,
                });
                (db.metas.len() - 1) as u32
            });
            match (a, b) {
                (IpAddr::V4(a), IpAddr::V4(b)) if a <= b => {
                    v4.push((u32::from(a), u32::from(b), m))
                }
                (IpAddr::V6(a), IpAddr::V6(b)) if a <= b => {
                    v6.push((u128::from(a), u128::from(b), m))
                }
                _ => bad += 1,
            }
        }
        if bad > 0 {
            tracing::warn!(bad, "asn db: skipped malformed lines");
        }
        db.v4 = Table::build(v4);
        db.v6 = Table::build(v6);
        db.names.shrink_to_fit();
        db.metas.shrink_to_fit();
        db.hosting.shrink_to_fit();
        Ok(db)
    }

    /// Look up an IP string (v4, v6, or v4-mapped v6). `None` for unparsable / unrouted.
    pub fn lookup(&self, ip: &str) -> Option<AsnInfo> {
        let ip: IpAddr = ip.trim().parse().ok()?;
        self.lookup_ip(ip)
    }

    pub fn lookup_ip(&self, ip: IpAddr) -> Option<AsnInfo> {
        let m = match ip {
            IpAddr::V4(v4) => self.v4.find(u32::from(v4)),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => self.v4.find(u32::from(v4)),
                None => self.v6.find(u128::from(v6)),
            },
        }?;
        let meta = self.metas[m as usize];
        let country = if meta.country == [0, 0] {
            String::new()
        } else {
            String::from_utf8_lossy(&meta.country).into_owned()
        };
        Some(AsnInfo {
            asn: meta.asn,
            country,
            name: self.names[meta.name as usize].to_string(),
            hosting: self.hosting[meta.name as usize],
        })
    }

    /// Number of routed ranges (v4 + v6).
    pub fn len(&self) -> usize {
        self.v4.starts.len() + self.v6.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const FIXTURE: &str = "\
1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET
1.0.1.0\t1.0.3.255\t0\tNone\tNot routed
1.0.4.0\t1.0.7.255\t4134\tCN\tCHINANET-BACKBONE No.31,Jin-rong Street
5.0.0.0\t5.0.0.255\t24940\tDE\tHETZNER-AS
5.0.1.0\t5.0.1.255\t24940\tFI\tHETZNER-AS
2001:db8::\t2001:db8::ffff\t16509\tUS\tAMAZON-02
2001:db8:1::\t2001:db8:1::ffff\t7922\tUS\tCOMCAST-7922
garbage line
";

    fn db() -> AsnDb {
        AsnDb::from_reader(BufReader::new(FIXTURE.as_bytes())).unwrap()
    }

    #[test]
    fn lookup_hits_misses_and_boundaries() {
        let db = db();
        assert_eq!(db.len(), 6);
        let cf = db.lookup("1.0.0.0").unwrap();
        assert_eq!((cf.asn, cf.country.as_str()), (13335, "US"));
        assert!(cf.hosting);
        assert_eq!(db.lookup("1.0.0.255").unwrap().asn, 13335);
        assert_eq!(db.lookup("1.0.1.0"), None); // asn 0
        assert_eq!(db.lookup("1.0.3.255"), None);
        let ct = db.lookup("1.0.7.255").unwrap();
        assert_eq!(ct.asn, 4134);
        assert!(!ct.hosting);
        assert_eq!(db.lookup("1.0.8.0"), None);
        assert_eq!(db.lookup("0.255.255.255"), None);
        assert_eq!(db.lookup("255.255.255.255"), None);
        assert_eq!(db.lookup("not an ip"), None);
        // same name, different country → distinct meta, shared name
        assert_eq!(db.lookup("5.0.1.1").unwrap().country, "FI");
        assert_eq!(db.lookup("5.0.0.1").unwrap().country, "DE");
        assert_eq!(db.names.len(), 5);
        // v4-mapped v6
        assert_eq!(db.lookup("::ffff:1.0.0.1").unwrap().asn, 13335);
    }

    #[test]
    fn lookup_v6() {
        let db = db();
        let a = db.lookup("2001:db8::").unwrap();
        assert_eq!(a.asn, 16509);
        assert!(a.hosting);
        assert_eq!(db.lookup("2001:db8::ffff").unwrap().asn, 16509);
        assert_eq!(db.lookup("2001:db8::1:0"), None);
        let c = db.lookup("2001:db8:1::5").unwrap();
        assert_eq!(c.name, "COMCAST-7922");
        assert!(!c.hosting);
        assert_eq!(db.lookup("::1"), None);
    }

    #[test]
    fn hosting_heuristic() {
        for n in [
            "AMAZON-02",
            "GOOGLE-CLOUD-PLATFORM",
            "DIGITALOCEAN-ASN",
            "OVH",
            "AS-CHOOPA",
            "M247 Europe SRL",
            "Datacamp Limited",
            "ALIBABA-CN-NET Hangzhou Alibaba Advertising Co.,Ltd.",
        ] {
            assert!(is_hosting_name(n), "{n}");
        }
        for n in [
            "CHINANET-BACKBONE No.31,Jin-rong Street",
            "CHINA169-BACKBONE CHINA UNICOM China169 Backbone",
            "CMNET-GD Guangdong Mobile Communication Co.Ltd.",
            "COMCAST-7922",
            "COLOMBIA TELECOMUNICACIONES S.A. ESP",
            "DTAG Deutsche Telekom AG",
            "LAWS-NET",
        ] {
            assert!(!is_hosting_name(n), "{n}");
        }
    }

    #[test]
    fn load_gz_and_plain() {
        let dir = tempfile::tempdir().unwrap();
        let gz = dir.path().join("a.tsv.gz");
        let mut enc = flate2::write::GzEncoder::new(
            std::fs::File::create(&gz).unwrap(),
            flate2::Compression::fast(),
        );
        enc.write_all(FIXTURE.as_bytes()).unwrap();
        enc.finish().unwrap();
        let plain = dir.path().join("a.tsv");
        std::fs::write(&plain, FIXTURE).unwrap();
        for p in [&gz, &plain] {
            let db = AsnDb::load(p).unwrap();
            assert_eq!(db.len(), 6);
            assert_eq!(db.lookup("5.0.0.128").unwrap().asn, 24940);
        }
        assert!(AsnDb::load(&dir.path().join("missing")).is_err());
    }

    /// Manual: `ASN_DB=/path/ip2asn-combined.tsv.gz cargo test -p gate-server --lib asn -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn load_real_file() {
        let Ok(p) = std::env::var("ASN_DB") else {
            return;
        };
        let t = std::time::Instant::now();
        let db = AsnDb::load(Path::new(&p)).unwrap();
        println!(
            "loaded {} ranges (v4 {}, v6 {}), {} metas, {} names in {:?}",
            db.len(),
            db.v4.starts.len(),
            db.v6.starts.len(),
            db.metas.len(),
            db.names.len(),
            t.elapsed()
        );
        for ip in ["8.8.8.8", "1.1.1.1", "114.114.114.114", "2606:4700::1111"] {
            println!("{ip} -> {:?}", db.lookup(ip));
        }
    }
}
