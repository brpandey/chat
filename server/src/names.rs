use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::ops::Deref;

use tokio::sync::RwLock;
// use tracing::info;

const USERS_MSG: &str = "Users currently online: ";

const DELIMITER: &str = "_";
const EXTRA_DELIMITER: &str = "!*#$";

pub struct NamesShared {
    names: Arc<RwLock<Names>>
}

/*
 Note:
 If name needs to be removed (one that has a duplicate),
 the name is removed from unique but still left in duplicate
 such that the counter values of previous names aren't reproduced
 Hence unique and duplicates both contain an owned type
 */

pub struct Names {
    unique: HashSet<String>,
    duplicates: HashMap<String, usize>,
    last_fixed: Option<String>,
}

impl NamesShared {
    pub fn new() -> Self {
        NamesShared {
            names: Arc::new(RwLock::new(Names::new()))
        }
    }

    pub fn clone(&self) -> Self {
        NamesShared {
            names: Arc::clone(&self.names)
        }
    }
}

// bring in auto deref functionality for NS
// to make Arc functionality accessible on NS type
impl Deref for NamesShared {
    type Target = Arc<RwLock<Names>>;

    fn deref(&self) -> &Self::Target {
        &self.names
    }
}

impl Names {
    pub fn new() -> Self {
        Names {
            unique: HashSet::new(),
            duplicates: HashMap::new(),
            last_fixed: None,
        }
    }

    pub fn to_list(&self) -> Vec<u8> {
        // construct list of users currently online by
        // grabbing names from chat names which is behind a RWLock
        let mut users_online: Vec<u8>;
        let mut list: Vec<Vec<u8>>;

        let set = &self.unique;
        list = Vec::with_capacity(set.len()*2);

        for n in set.iter() {
            list.push(n.clone().into_bytes());
            list.push(vec![b' ']);
        }

        let mut joined: Vec<u8> = list.into_iter().flatten().collect();
        users_online = USERS_MSG.as_bytes().to_vec();
        users_online.append(&mut joined);
        users_online
    }

    // Follows HashSet insert semantics returning bool, e.g.
    // If the set did not previously contain this value, true is returned.
    // If the set already contained this value, false is returned.
    pub fn insert(&mut self, mut name: String) -> bool {
        // remove names ending with special character
        if name.ends_with("_") {
            name = name.trim_end_matches('_').to_owned();
        }

        // if never seen insert and return
        if !self.unique.contains(&name) {
            self.unique.insert(name);
            true
        } else { // if name is dup, check dups map for next counter

            let mut value = 2;
            if let Some(v) = self.duplicates.get_mut(&name) {
                *v += 1;
                value = *v;
            } else {
                self.duplicates.insert(name.clone(), value);
            }

            let v_string = value.to_string();
            // append counter value to name e.g. if "anna" already present then new name is "anna_2"
            name.push_str(DELIMITER);
            name.push_str(&v_string);

            // update unique with new name, if THAT is already found
            // then just append a strange string and call it a day
            if !self.unique.insert(name.clone()) {
                name.push_str(EXTRA_DELIMITER);
                self.unique.insert(name.clone());
            }

            self.last_fixed.replace(name);
            false
        }
    }

    // Used in conjunction with insert, if insert is false, caller must call this
    // function to get new name value e.g. "anna_2" updated after the collision
    pub fn last_collision(&mut self) -> Option<String> {
        self.last_fixed.take()
    }

    pub fn remove(&mut self, name: &String) -> bool {
        // only remove from hashset,
        // if there was a duplicate entry just leave it with its
        // counter value
        self.unique.remove(name)
    }
}
