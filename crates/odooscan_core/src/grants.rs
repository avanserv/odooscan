//! The access rows that let public and portal users at a model, as the scope of a module loads them.

use std::path::Path;

use rustc_hash::FxHashMap;

use crate::domain;
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

/// Which of portal and public users the scanned rows let create an attachment they may make
/// `public`. A 20.0 `ir.access` row grants only the records its domain holds, so one refusing
/// `public=True` grants none (`odoo/addons/base/models/ir_access.py:523-529`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AttachmentGrant {
    pub portal: bool,
    pub public: bool,
}

impl AttachmentGrant {
    /// The portal and public create rows on `ir.attachment`, less those whose domain refuses
    /// `public=True`.
    pub fn of(grants: &Grants) -> Self {
        let mut grant = Self::default();
        let rows = grants.public("ir.attachment", creates);
        for g in rows.filter(|g| {
            !g.row
                .domain
                .as_deref()
                .is_some_and(|d| domain::refuses(d, "public", true))
        }) {
            match g.group.as_deref() {
                None | Some(EVERYONE) => (grant.portal, grant.public) = (true, true),
                Some("base.group_portal") => grant.portal = true,
                Some("base.group_public") => grant.public = true,
                Some(_) => {}
            }
        }
        grant
    }

    /// Whether any portal or public user may create an attachment.
    pub fn any(self) -> bool {
        self.portal || self.public
    }

    /// Whether the grant reaches the user of a request. An `auth="user"` route refuses the public
    /// user (18.0 `odoo/addons/base/models/ir_http.py:246-248`), so only a portal grant reaches it.
    pub fn reaches(self, signed_in: bool) -> bool {
        self.portal || (!signed_in && self.public)
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

#[cfg(test)]
mod tests {
    use super::AttachmentGrant;

    #[test]
    fn a_public_grant_reaches_no_signed_in_user() {
        let grant = |portal, public| AttachmentGrant { portal, public };
        for (portal, public, signed_in, reaches) in [
            (true, false, true, true),
            (true, false, false, true),
            (false, true, true, false),
            (false, true, false, true),
            (true, true, true, true),
            (false, false, false, false),
            (false, false, true, false),
        ] {
            assert_eq!(
                grant(portal, public).reaches(signed_in),
                reaches,
                "{portal} {public} {signed_in}"
            );
        }
    }
}
