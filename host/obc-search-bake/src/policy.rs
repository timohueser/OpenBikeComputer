use super::features::Feature;
use regex::Regex;
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};

struct Country {
    postcode: Option<(Regex, String)>,
    extent: f64,
    names: BTreeMap<String, String>,
}

pub struct Policy {
    levels: Vec<Value>,
    countries: BTreeMap<String, Country>,
}

impl Policy {
    pub fn read(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        Self::parse(&fs::read_to_string(path)?)
    }

    fn parse(data: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let data: Value = serde_json::from_str(data)?;
        let levels = data["levels"].as_array().ok_or("Missing address levels")?.clone();
        let mut countries = BTreeMap::new();
        let captures_pattern = Regex::new(r"\\(?:g<(\d+)>|(\d+))")?;
        for (code, value) in data["countries"].as_object().ok_or("Missing countries")? {
            let pc = &value["postcode"];
            let postcode = if pc == false {
                None
            } else {
                let pattern = pc["pattern"].as_str().unwrap_or(".*").replace('d', "[0-9]").replace('l', "[A-Z]");
                let pattern = Regex::new(&format!("^(?:{}[ -]?)?({pattern})$", code.to_uppercase()))?;
                // The outer capture holds the code. Configuration captures start at two.
                let output = pc["output"].as_str().unwrap_or("\\g<0>");
                let output = captures_pattern
                    .replace_all(output, |c: &regex::Captures| {
                        format!(
                            "${{{}}}",
                            c.get(1).or_else(|| c.get(2)).unwrap().as_str().parse::<usize>().unwrap() + 1
                        )
                    })
                    .into_owned();
                Some((pattern, output))
            };
            let names = value["names"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect();
            countries.insert(code.clone(), Country { postcode, names, extent: pc["extent"].as_f64().unwrap_or(5000.) });
        }
        Ok(Self { levels, countries })
    }

    pub fn has_country(&self, code: &str) -> bool {
        self.countries.contains_key(code)
    }

    pub fn postcode(&self, code: &str, country: &str) -> Option<String> {
        let (pattern, output) = self.countries.get(country)?.postcode.as_ref()?;
        let text = code.trim().to_uppercase();
        let captures = pattern.captures(&text)?;
        if captures[1].chars().all(|c| matches!(c, '0' | '-' | ' ')) {
            return None;
        }
        let mut result = String::new();
        captures.expand(output, &mut result);
        Some(result)
    }

    pub fn postcode_extent(&self, country: &str) -> f64 {
        self.countries.get(country).map(|c| c.extent).unwrap_or(5000.)
    }

    pub fn country_names(&self, country: &str) -> impl Iterator<Item = (&str, &str)> {
        self.countries.get(country).into_iter().flat_map(|c| c.names.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    pub fn ranks(&self, feature: &Feature, country: &str) -> (u8, u8) {
        let class = if feature.tag("boundary") == "administrative" {
            Some("boundary")
        } else {
            [
                "place",
                "highway",
                "amenity",
                "shop",
                "tourism",
                "office",
                "craft",
                "boundary",
                "leisure",
                "natural",
                "water",
                "waterway",
                "mountain_pass",
                "historic",
                "landuse",
            ]
            .into_iter()
            .find(|k| !feature.tag(k).is_empty())
        };
        let Some(class) = class else { return (30, 30) };
        if class == "highway" && matches!(feature.source, osmpbfreader::OsmId::Node(_)) {
            return (30, 30);
        }
        if class == "landuse" && !matches!(feature.geometry, geo::Geometry::Polygon(_) | geo::Geometry::MultiPolygon(_))
        {
            return (30, 30);
        }
        let value = if class == "boundary" && feature.tag(class) == "administrative" {
            format!("administrative{}", feature.tag("admin_level"))
        } else {
            feature.tag(class).to_string()
        };
        let mut ranks = self.for_type(class, &value, country);
        if class == "place" && value == "city" && feature.tag("capital") == "yes" {
            ranks.0 -= 1;
        }
        ranks
    }

    pub fn place_rank(&self, place: &str, country: &str) -> u8 {
        self.for_type("place", place, country).1
    }

    fn for_type(&self, class: &str, value: &str, country: &str) -> (u8, u8) {
        let mut result = (30, 30);
        for level in &self.levels {
            if level["countries"].as_array().is_some_and(|codes| !codes.iter().any(|c| c == country)) {
                continue;
            }
            let tags = &level["tags"][class];
            let rank = tags.get(value).or_else(|| tags.get(""));
            if let Some(rank) = rank {
                result = if let Some(rank) = rank.as_u64() {
                    (rank as u8, rank as u8)
                } else {
                    (rank[0].as_u64().unwrap_or(30) as u8, rank[1].as_u64().unwrap_or(30) as u8)
                };
            }
        }
        result
    }

    #[cfg(test)]
    pub fn test_policy() -> Self {
        Self::parse(r#"{"levels":[{"tags":{"highway":{"":26},"place":{"city":16},"boundary":{"administrative8":16}}}],"countries":{"de":{"postcode":{"pattern":"ddddd"}},"us":{"postcode":{"pattern":"(ddddd)(?:-dddd)?","output":"\\1"}},"ch":{"postcode":{"pattern":"dddd","extent":3000}},"jp":{"postcode":{"pattern":"(ddd)-?(dddd)","output":"\\1-\\2"}},"ad":{"postcode":{"pattern":"(ddd)","output":"AD\\1"}},"ae":{"postcode":false}}}"#).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn postcodes_apply_country_prefixes_formats_and_exclusions() {
        let p = Policy::test_policy();
        assert_eq!(p.postcode("US 80481-1234", "us").as_deref(), Some("80481"));
        assert_eq!(p.postcode("jp 1234567", "jp").as_deref(), Some("123-4567"));
        assert_eq!(p.postcode("AD100", "ad").as_deref(), Some("AD100"));
        assert_eq!(p.postcode("00000", "de"), None);
        assert_eq!(p.postcode("12345", "ae"), None);
        assert_eq!(p.postcode("12345", "unknown"), None);
    }
}
