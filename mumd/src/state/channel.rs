use crate::state::State;

use mumble_protocol::control::msgs;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::convert::Into;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Channel {
    description: Option<String>,
    links: Vec<u32>, //TODO: is this ever used?
    max_users: u32,
    name: String,
    parent: Option<u32>,
    position: i32,
}

impl Channel {
    pub fn new(mut msg: Box<msgs::ChannelState>) -> Self {
        Self {
            description: if msg.has_description() {
                Some(msg.take_description())
            } else {
                None
            },
            links: Vec::new(),
            max_users: msg.get_max_users(),
            name: msg.take_name(),
            parent: if msg.has_parent() {
                Some(msg.get_parent())
            } else {
                None
            },
            position: msg.get_position(),
        }
    }

    pub fn parse_state(&mut self, mut msg: Box<msgs::ChannelState>) {
        if msg.has_description() {
            self.description = Some(msg.take_description());
        }
        self.links = msg.take_links();
        if msg.has_max_users() {
            self.max_users = msg.get_max_users();
        }
        if msg.has_name() {
            self.name = msg.take_name();
        }
        if msg.has_parent() {
            self.parent = Some(msg.get_parent());
        }
        if msg.has_position() {
            self.position = msg.get_position();
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self, channels: &HashMap<u32, Channel>) -> String {
        match &self.parent {
            Some(t) => format!("{}/{}", channels.get(t).unwrap().path(channels), self.name),
            None => self.name.clone(),
        }
    }
}

//struct ProtoTree<'a> {
//    channel: Option<&'a Channel>,
//    children: HashMap<u32, ProtoTree<'a>>,
//    users: Vec<&'a User>,
//}
//
//impl<'a> ProtoTree<'a> {
//    fn walk_and_add(
//        &mut self,
//        channel: &'a Channel,
//        users: &HashMap<u32, Vec<&'a User>>,
//        walk: &[u32],
//    ) {
//        match walk {
//            [] => unreachable!("Walks should always have at least one element"),
//            &[node] => {
//                let pt = self.children.entry(node).or_insert(ProtoTree {
//                    channel: None,
//                    children: HashMap::new(),
//                    users: Vec::new(),
//                });
//                pt.channel = Some(channel);
//                pt.users = users.get(&node).cloned().unwrap_or_default();
//            }
//            longer => {
//                self.children
//                    .entry(longer[0])
//                    .or_insert(ProtoTree {
//                        channel: None,
//                        children: HashMap::new(),
//                        users: Vec::new(),
//                    })
//                    .walk_and_add(channel, users, &walk[1..]);
//            }
//        }
//    }
//}
//
//impl<'a> From<&ProtoTree<'a>> for mumlib::state::Channel {
//    fn from(tree: &ProtoTree<'a>) -> Self {
//        let mut channel = mumlib::state::Channel::from(tree.channel.unwrap());
//        let mut children = tree
//            .children
//            .iter()
//            .map(|e| {
//                (
//                    e.1.channel.unwrap().position,
//                    mumlib::state::Channel::from(e.1),
//                )
//            })
//            .collect::<Vec<_>>();
//        children.sort_by_key(|e| (e.0, e.1.name.clone()));
//        channel.children = children.into_iter().map(|e| e.1).collect();
//        channel.users = tree.users.iter().map(|e| (*e).into()).collect();
//        channel
//    }
//}

//TODO what is this??? This obviously got out of hand, fix this
pub async fn into_channel(state: &Arc<RwLock<State>>) -> mumlib::state::Channel {
    // convert all channels/users to mumlib channels/users and find the root
    let mut root: Option<(u32, mumlib::state::Channel)> = None;
    let mut channels: HashMap<u32, (u32, mumlib::state::Channel)> =
        HashMap::new();
    { // aquire state read lock
        let state_lock = state.read().await;
        { // acquire channel read lock
            let channels_lock = state_lock.channels.read().await;
            for (channel_id, channel) in channels_lock.iter() {
                let channel = channel.read().await;
                let channel = &*channel;
                match channel.parent {
                    None => {
                        if root.is_none() {
                            root = Some((*channel_id, channel.into()));
                        } else {
                            //TODO: Error: two roots
                            unimplemented!();
                        }
                    }
                    Some(parent_id) => {
                        channels.insert(
                            *channel_id,
                            (parent_id, channel.into())
                        );
                    }
                }
            }
        } // release channel read lock

        //populate the mumlib::Channel users vec
        { // acquire users read lock
            let users = state_lock.users.read().await;
            for user in users.values() {
                let user = user.read().await;
                let user = &*user; //TODO there is a better way?
                let channel_id = user.channel();
                let channel = channels.get_mut(&channel_id).unwrap();
                let new_user = user.into();
                channel.1.users.push(new_user);
            }
        } // release users read lock
    } // release state read lock

    //TODO better check if root was found
    let (root_id, root) = root.unwrap();

    struct IdTree {
        id: u32,
        parent: Weak<Mutex<IdTree>>,
        children: Vec<Rc<Mutex<IdTree>>>,
        is_populated: bool,
        is_done: bool,
        channel: Option<mumlib::state::Channel>,
    }
    let id_tree = Rc::new(Mutex::new(IdTree {
        id: root_id,
        parent: Weak::new(),
        children: vec![],
        is_populated: false,
        is_done: false,
        channel: Some(root),
    }));
    //populate the id tree
    let mut id_tree_current = Rc::clone(&id_tree);
    loop {
        //if done, goes to the parent
        let mut id_tree_current_lock = id_tree_current.lock().unwrap();
        if id_tree_current_lock.is_done {
            match id_tree_current_lock.parent.upgrade() {
                Some(x) => {
                    drop(id_tree_current_lock);
                    id_tree_current = Rc::clone(&x);
                    continue;
                },
                //no parent, this is root, it is populated and done
                None => break,
            }
        }
        if !id_tree_current_lock.is_populated {
            //check all channels witch one are children from this one
            let mut channels_move = vec![];
            for (channel_id, (channel_pid, _)) in channels.iter() {
                if id_tree_current_lock.id == *channel_pid {
                    channels_move.push(*channel_id);
                }
            }
            //move the used channels to the tree
            for move_id in channels_move {
                let (_, channel) = channels.remove(&move_id).unwrap();
                id_tree_current_lock.children.push(
                    Rc::new(Mutex::new(IdTree {
                        id: move_id,
                        parent: Rc::downgrade(&id_tree_current),
                        children: vec![],
                        is_populated: false,
                        is_done: false,
                        channel: Some(channel),
                    }
                )));
            }
            id_tree_current_lock.is_populated = true;
        }

        //process the children
        let next = match id_tree_current_lock.children.iter().find(|x| !x.lock().unwrap().is_done) {
            //this children is not done, process him next
            Some(x) => {
                Rc::clone(x)
            },
            //if all children are done, then he is also done
            None => {
                id_tree_current_lock.is_done = true;
                //conver this node into final
                let children = id_tree_current_lock.children.drain(..)
                    .map(|x| {x.lock().unwrap().channel.take().unwrap()})
                    .collect::<Vec<_>>();
                id_tree_current_lock.channel.as_mut().unwrap().children = children;
                continue;
            }
        };
        drop(id_tree_current_lock);
        id_tree_current = next;
    }

    let mut lock = id_tree.lock().unwrap();
    let ret = lock.channel.take().unwrap();
    ret
}

impl From<&Channel> for mumlib::state::Channel {
    fn from(channel: &Channel) -> Self {
        mumlib::state::Channel {
            description: channel.description.clone(),
            links: Vec::new(),
            max_users: channel.max_users,
            name: channel.name.clone(),
            children: Vec::new(),
            users: Vec::new(),
        }
    }
}
