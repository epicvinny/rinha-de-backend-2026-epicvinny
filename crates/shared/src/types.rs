use serde::{Deserialize, Serialize};

pub type Vec14F = [f64; 14];
pub type Vec16I = [i16; 16];

#[derive(Debug, Clone)]
pub struct KnownMerchants<'a> {
    pub items: [&'a str; 8],
    pub len: usize,
}

impl<'a> Default for KnownMerchants<'a> {
    fn default() -> Self {
        KnownMerchants {
            items: [""; 8],
            len: 0,
        }
    }
}

impl<'a> KnownMerchants<'a> {
    pub fn from_slice(slice: &[&'a str]) -> Self {
        let mut items = [""; 8];
        let len = slice.len().min(8);
        items[..len].copy_from_slice(&slice[..len]);
        KnownMerchants { items, len }
    }

    #[inline]
    pub fn contains(&self, merchant_id: &str) -> bool {
        for i in 0..self.len {
            if self.items[i] == merchant_id {
                return true;
            }
        }
        false
    }
}

impl<'de: 'a, 'a> serde::Deserialize<'de> for KnownMerchants<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor<'a> {
            marker: std::marker::PhantomData<&'a str>,
        }
        impl<'de: 'a, 'a> serde::de::Visitor<'de> for Visitor<'a> {
            type Value = KnownMerchants<'a>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a sequence of strings")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut items = [""; 8];
                let mut len = 0;
                while let Some(val) = seq.next_element::<&'a str>()? {
                    if len < 8 {
                        items[len] = val;
                        len += 1;
                    }
                }
                Ok(KnownMerchants { items, len })
            }
        }
        deserializer.deserialize_seq(Visitor {
            marker: std::marker::PhantomData,
        })
    }
}

impl<'a> serde::Serialize for KnownMerchants<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.len))?;
        for i in 0..self.len {
            seq.serialize_element(&self.items[i])?;
        }
        seq.end()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Transaction<'a> {
    pub amount: f64,
    pub installments: u32,
    pub requested_at: &'a str,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Customer<'a> {
    pub avg_amount: f64,
    pub tx_count_24h: u32,
    #[serde(borrow)]
    pub known_merchants: KnownMerchants<'a>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Merchant<'a> {
    pub id: &'a str,
    pub mcc: &'a str,
    pub avg_amount: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Terminal {
    pub is_online: bool,
    pub card_present: bool,
    pub km_from_home: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LastTransaction<'a> {
    pub timestamp: &'a str,
    pub km_from_current: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Payload<'a> {
    #[serde(default)]
    pub id: &'a str,
    #[serde(borrow)]
    pub transaction: Transaction<'a>,
    #[serde(borrow)]
    pub customer: Customer<'a>,
    #[serde(borrow)]
    pub merchant: Merchant<'a>,
    pub terminal: Terminal,
    #[serde(borrow)]
    pub last_transaction: Option<LastTransaction<'a>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Reference {
    pub vector: Vec<f64>,
    pub label: String,
}
