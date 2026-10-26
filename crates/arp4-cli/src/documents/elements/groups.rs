//! The groups of the table cells of a page, held compactly: a page has a group
//! for each cell of its tables, and the cells of a record or a column share
//! most of what their groups hold. Each distinct group is kept once, and a
//! cell names its group by number, by the numbers of its row and column
//! identities rather than by its ID.
use super::{Group, cell_axes, cell_id, plain};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

/// The identities of rows or of columns, each numbered in order of first use.
#[derive(Clone, Debug, Default)]
struct Names {
    list: Vec<Arc<str>>,
    numbers: HashMap<Arc<str>, u32>,
}

impl Names {
    fn number(&mut self, name: &str) -> u32 {
        if let Some(&number) = self.numbers.get(name) {
            return number;
        }
        let number = self.list.len() as u32;
        let name: Arc<str> = name.into();
        self.list.push(name.clone());
        self.numbers.insert(name, number);
        number
    }

    fn find(&self, name: &str) -> Option<u32> {
        self.numbers.get(name).copied()
    }
}

/// The groups of the table cells of a page, by cell ID.
#[derive(Clone, Debug, Default)]
pub(in crate::documents) struct Groups {
    distinct: Vec<Group>,
    numbers: HashMap<Group, u32>,
    rows: Names,
    columns: Names,
    /// The group of each cell, by the numbers of its row and column.
    cells: HashMap<(u32, u32), u32>,
    /// The group of each cell whose ID names no row and column.
    others: HashMap<Box<str>, u32>,
}

/// The row and column identities the cell `id` names, as [`cell_axes`] reads
/// them: borrowed from an ID that escapes no byte, which a cell ID made of
/// the identities of an original's rows and columns is.
fn axes(id: &str) -> Option<(Cow<'_, str>, Cow<'_, str>)> {
    if let Some((row, column)) = id
        .strip_prefix("cell_")
        .and_then(|rest| rest.split_once('_'))
        && row.bytes().chain(column.bytes()).all(plain)
    {
        return Some((Cow::Borrowed(row), Cow::Borrowed(column)));
    }
    cell_axes(id).map(|(row, column)| (Cow::Owned(row), Cow::Owned(column)))
}

impl Groups {
    pub(in crate::documents) fn len(&self) -> usize {
        self.cells.len() + self.others.len()
    }

    /// The group of the cell `id`.
    pub(in crate::documents) fn get(&self, id: &str) -> Option<&Group> {
        let number = match axes(id) {
            Some((row, column)) => self.number_at(&row, &column)?,
            None => *self.others.get(id)?,
        };
        Some(&self.distinct[number as usize])
    }

    /// The group of the cell of the row `row` and the column `column`.
    pub(in crate::documents) fn get_at(&self, row: &str, column: &str) -> Option<&Group> {
        self.number_at(row, column)
            .map(|number| &self.distinct[number as usize])
    }

    fn number_at(&self, row: &str, column: &str) -> Option<u32> {
        let key = (self.rows.find(row)?, self.columns.find(column)?);
        self.cells.get(&key).copied()
    }

    pub(in crate::documents) fn contains_key(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    /// Gives the cell `id` the group `group`.
    pub(in crate::documents) fn insert(&mut self, id: &str, group: Group) {
        let number = self.number(group);
        match axes(id) {
            Some((row, column)) => {
                let key = (self.rows.number(&row), self.columns.number(&column));
                self.cells.insert(key, number);
            }
            None => {
                self.others.insert(id.into(), number);
            }
        }
    }

    /// Gives the cell of the row `row` and the column `column` the group `group`.
    pub(in crate::documents) fn insert_at(&mut self, row: &str, column: &str, group: Group) {
        let number = self.number(group);
        let key = (self.rows.number(row), self.columns.number(column));
        self.cells.insert(key, number);
    }

    /// Changes the group of the cell of the row `row` and the column `column`,
    /// a group without a table when it has none, as `change` does. The groups
    /// of the other cells sharing it stay as they are.
    pub(in crate::documents) fn update_at(
        &mut self,
        row: &str,
        column: &str,
        change: impl FnOnce(&mut Group),
    ) {
        let mut group = self.get_at(row, column).cloned().unwrap_or_default();
        change(&mut group);
        self.insert_at(row, column, group);
    }

    fn number(&mut self, group: Group) -> u32 {
        if let Some(&number) = self.numbers.get(&group) {
            return number;
        }
        let number = self.distinct.len() as u32;
        self.distinct.push(group.clone());
        self.numbers.insert(group, number);
        number
    }

    /// The groups of the cells, in the order of their IDs.
    pub(in crate::documents) fn iter(&self) -> impl Iterator<Item = (String, &Group)> {
        let mut cells: Vec<(String, u32)> = self
            .cells
            .iter()
            .map(|(&(row, column), &number)| {
                let id = cell_id(
                    &self.rows.list[row as usize],
                    &self.columns.list[column as usize],
                );
                (id, number)
            })
            .chain(
                self.others
                    .iter()
                    .map(|(id, &number)| (id.to_string(), number)),
            )
            .collect();
        cells.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        cells
            .into_iter()
            .map(|(id, number)| (id, &self.distinct[number as usize]))
    }

    /// Lets go of the room left for more cells and groups.
    pub(in crate::documents) fn shrink_to_fit(&mut self) {
        self.distinct = crate::data::exact(std::mem::take(&mut self.distinct));
        self.numbers.shrink_to_fit();
        self.cells.shrink_to_fit();
        self.others.shrink_to_fit();
    }
}

/// Groups are alike when they give each cell the same group, however they
/// number them.
impl PartialEq for Groups {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl FromIterator<(String, Group)> for Groups {
    fn from_iter<I: IntoIterator<Item = (String, Group)>>(cells: I) -> Self {
        let mut groups = Self::default();
        for (id, group) in cells {
            groups.insert(&id, group);
        }
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(table: &str) -> Group {
        Group {
            table: Some(table.into()),
            ..Group::default()
        }
    }

    /// The cells of a table share their group, a cell is found by its ID or
    /// its row and column, and an ID naming no cell is kept as it is.
    #[test]
    fn cells_share_their_groups() {
        let mut groups = Groups::default();
        groups.insert("cell_r1_A", group("table_x"));
        groups.insert_at("r1", "B", group("table_x"));
        groups.insert("cell_r%20_A", group("table_y"));
        groups.insert("shape_1", group("table_z"));
        assert_eq!(groups.distinct.len(), 3);
        assert_eq!(groups.len(), 4);
        assert_eq!(groups.get("cell_r1_B"), Some(&group("table_x")));
        assert_eq!(groups.get_at("r ", "A"), Some(&group("table_y")));
        assert_eq!(groups.get("shape_1"), Some(&group("table_z")));
        assert!(!groups.contains_key("cell_r2_A"));
        groups.update_at("r1", "A", |g| g.heading = true);
        assert!(groups.get("cell_r1_A").unwrap().heading);
        assert!(!groups.get("cell_r1_B").unwrap().heading);
        let ids: Vec<String> = groups.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["cell_r%20_A", "cell_r1_A", "cell_r1_B", "shape_1"]);
    }
}
