use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub(crate) id: String,
    pub(crate) name: String,
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\" ({})", self.name, self.id)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DefaultEndpoints {
    pub(crate) console: Option<Endpoint>,
    pub(crate) communications: Option<Endpoint>,
}

impl DefaultEndpoints {
    pub(crate) fn to_capture(&self) -> Vec<(&'static str, Endpoint)> {
        let mut endpoints = Vec::new();
        if let Some(console) = &self.console {
            endpoints.push(("console", console.clone()));
        }
        if let Some(communications) = &self.communications {
            if self.console.as_ref().map(|c| &c.id) != Some(&communications.id) {
                endpoints.push(("communications", communications.clone()));
            }
        }
        endpoints
    }
}

impl fmt::Display for DefaultEndpoints {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let describe = |endpoint: &Option<Endpoint>| {
            endpoint
                .as_ref()
                .map_or_else(|| "none".to_string(), Endpoint::to_string)
        };
        write!(
            f,
            "console={} communications={}",
            describe(&self.console),
            describe(&self.communications)
        )
    }
}
