//! The access rows that let public and portal users at a model, as the scope of a module loads them.

use std::path::Path;

use rustc_hash::FxHashMap;

use crate::facts::{AccessRow, Perms};
use crate::index::{Index, Scope, full_id, loaded_data};
use crate::series::Series;

/// The first series with `base.group_everyone`, which every user is in: saas~19.1 defines it in
/// `odoo/addons/base/security/base_groups.xml` (`:92` on 20.0), 19.0 does not.
const GROUP_EVERYONE: Series = Series { major: 19, minor: 1 };

/// The groups of public and portal users in every series, as full XML ids.
pub const PUBLIC_GROUPS: [&str; 2] = ["base.group_portal", "base.group_public"];

/// The group every user is in, portal and public ones included, from saas~19.1.
pub const EVERYONE: &str = "base.group_everyone";

/// Which operations of a row a check asks for.
pub type Kind = fn(Perms) -> bool;

/// Whether a group, as a full XML id, holds public or portal users in `series`.
pub fn public_group(group: &str, series: Option<Series>) -> bool {
    PUBLIC_GROUPS.contains(&group) || (group == EVERYONE && series.is_none_or(|s| s >= GROUP_EVERYONE))
}

/// An access row that grants public or portal users.
pub struct Grant<'m> {
    pub module: usize,
    /// The group as a full XML id, None for every user.
    pub group: Option<String>,
    pub row: &'m AccessRow,
    pub path: &'m Path,
    /// Where it loads in its module: the file's rank, then the line. A file only code loads is last.
    order: (usize, u32),
}

impl Grant<'_> {
    /// Whom the row grants, as a message names them.
    pub fn whom(&self) -> String {
        match &self.group {
            Some(group) => group.clone(),
            None => "every user, public and portal included,".to_owned(),
        }
    }
}

/// The public grants of the rows in scope, by model.
pub struct Grants<'m> {
    by_model: FxHashMap<&'m str, Vec<Grant<'m>>>,
}

impl<'m> Grants<'m> {
    /// The rows of the data files the modules of `scope` load, short of demo data, which a database
    /// may skip. `model_access` says whether `series` reads `ir.model.access`, whose row without a
    /// group grants every user. A row whose id is written twice may hold another value.
    pub fn of(index: &Index<'m>, scope: &Scope, series: Option<Series>, model_access: bool) -> Self {
        let mut by_model: FxHashMap<&'m str, Vec<Grant<'m>>> = FxHashMap::default();
        for (mi, module) in index.modules.iter().enumerate().filter(|(mi, _)| scope.sees(*mi)) {
            let files = loaded_data(module, module.series.or(series)).filter(|f| !f.demo);
            for file in files {
                let rows = file.facts.access.iter().filter(|r| model_access || !r.model_access);
                let once = |r: &&AccessRow| {
                    let id = r.id.as_ref().map(|id| full_id(&module.name, id));
                    id.is_none_or(|id| index.xmlid_defs(&id, series).len() < 2)
                };
                for row in rows.filter(once) {
                    let group = row.group.as_ref().map(|g| full_id(&module.name, g));
                    if group.as_ref().is_some_and(|g| !public_group(g, series)) || row.grants == Perms::default() {
                        continue;
                    }
                    let model = if row.by_name {
                        Some(row.model.as_str())
                    } else {
                        index.model_of_xmlid(&full_id(&module.name, &row.model))
                    };
                    let Some(model) = model else { continue };
                    by_model.entry(model).or_default().push(Grant {
                        module: mi,
                        group,
                        row,
                        path: &file.path,
                        order: (file.rank.unwrap_or(usize::MAX), row.pos.line),
                    });
                }
            }
        }
        Self { by_model }
    }

    /// The public grants of `model` that `kind` picks.
    pub fn public(&self, model: &str, kind: Kind) -> impl Iterator<Item = &Grant<'m>> {
        let grants = self.by_model.get(model).map_or(&[][..], Vec::as_slice);
        grants.iter().filter(move |g| kind(g.row.grants))
    }

    /// Each model whose public grants that `kind` picks all load in module `mi`, with the first:
    /// the row that lets public or portal users at the model.
    pub fn introduced_in(&self, mi: usize, kind: Kind) -> impl Iterator<Item = (&'m str, &Grant<'m>)> {
        self.by_model.keys().filter_map(move |&model| {
            if self.public(model, kind).any(|g| g.module != mi) {
                return None;
            }
            Some((model, self.public(model, kind).min_by_key(|g| g.order)?))
        })
    }
}

/// Public and portal users may read.
pub fn reads(perms: Perms) -> bool {
    perms.read
}

/// Public and portal users may store a value.
pub fn stores(perms: Perms) -> bool {
    perms.write || perms.create
}

/// Public and portal users may create a record.
pub fn creates(perms: Perms) -> bool {
    perms.create
}
