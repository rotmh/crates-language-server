use std::collections::{HashMap, hash_map};

use tower_lsp::lsp_types::CompletionItem;

use crate::{crates, parse};

pub fn version_completions(latest: crates::Latest) -> Vec<CompletionItem> {
    let version = latest.version;

    let mut comps = vec![
        CompletionItem::new_simple(
            format!("{}.{}.{}", version.major, version.minor, version.patch),
            "patch".to_owned(),
        ),
        CompletionItem::new_simple(
            format!("{}.{}", version.major, version.minor),
            "minor".to_owned(),
        ),
        CompletionItem::new_simple(
            format!("{}", version.major),
            "major".to_owned(),
        ),
    ];

    // this is often not the case, so it's not that bad the we are
    // inserting here (which is O(N)).
    if !(version.pre.is_empty() && version.build.is_empty()) {
        let full = CompletionItem::new_simple(
            version.to_string(),
            "latest".to_owned(),
        );
        comps.insert(0, full);
    }

    comps
}

pub fn format_vec(vec: &[String]) -> String {
    format!("[ {} ]", vec.join(", "))
}

pub fn features_completions(
    dependency: &parse::Dependency,
    latest: crates::Latest,
) -> Vec<CompletionItem> {
    let features = dependency.features.as_ref();
    let already_used = |name: &str| {
        features.is_some_and(|f| f.iter().any(|f| f.value == name))
    };

    // TODO: make the completions _replace_ the current content of the feature.

    if let Some(available_features) = latest.features {
        available_features
            .into_iter()
            .filter(|(name, _)| !already_used(name))
            .map(|(name, f)| CompletionItem::new_simple(name, format_vec(&f)))
            .collect()
    } else {
        Vec::new()
    }
}

pub fn format_feature_hover(
    feature: &str,
    feature_description: &[String],
) -> String {
    format!("{}\n\n{}", feature, format_vec(feature_description))
}

pub fn format_name_hover(name: &str, latest: crates::Latest) -> String {
    let header = format!("{}: {}", name, latest.version);

    // Format the features like so:
    //
    //   [ feat1, feat2, feat3 ]
    let features = latest
        .features
        .as_ref()
        .map(HashMap::keys)
        .map(hash_map::Keys::into_iter)
        .map(|f| f.map(String::as_str))
        .map(Iterator::collect::<Vec<_>>)
        .map(|f| f.join(", "));
    let features = features
        .filter(|f| !f.is_empty())
        .map(|f| format!("[ {} ]", f));

    [Some(header), features, latest.description]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n")
}
