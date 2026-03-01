// arc_router/src/lib.rs
#![forbid(unsafe_code)]

pub type RouteId = u32;

#[derive(Clone, Debug, Default)]
pub struct Router {
    nodes: Vec<Node>,
}

#[derive(Clone, Debug, Default)]
struct Node {
    // sorted by byte for binary_search
    children: Vec<(u8, usize)>,
    // all routes registered on this prefix node
    routes: Vec<RouteId>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            nodes: vec![Node::default()], // root
        }
    }

    /// Insert a route for a path prefix (bytes). Typical: b"/", b"/foo", b"/foo/bar".
    pub fn insert_prefix(&mut self, prefix: &[u8], route: RouteId) {
        let mut cur = 0usize;
        for &b in prefix {
            let next = {
                let node = &mut self.nodes[cur];
                match node.children.binary_search_by_key(&b, |(c, _)| *c) {
                    Ok(pos) => node.children[pos].1,
                    Err(pos) => {
                        let idx = self.nodes.len();
                        self.nodes.push(Node::default());
                        node.children.insert(pos, (b, idx));
                        idx
                    }
                }
            };
            cur = next;
        }

        let node = &mut self.nodes[cur];
        if !node.routes.contains(&route) {
            node.routes.push(route);
        }
    }

    /// Iterate all candidate routes for `path` by walking the trie and yielding routes
    /// on every visited prefix node (so "/foo/bar" yields routes at "/", "/foo", "/foo/bar"...).
    #[inline]
    pub fn for_each_candidate<F: FnMut(RouteId)>(&self, path: &[u8], mut f: F) {
        let mut cur = 0usize;

        // root candidates ("/" or global routes)
        for &rid in self.nodes[cur].routes.iter() {
            f(rid);
        }

        for &b in path {
            let next = match self.nodes[cur]
                .children
                .binary_search_by_key(&b, |(c, _)| *c)
            {
                Ok(pos) => self.nodes[cur].children[pos].1,
                Err(_) => break,
            };
            cur = next;

            for &rid in self.nodes[cur].routes.iter() {
                f(rid);
            }
        }
    }

    /// Backward-compat: return the first candidate if you still need single-route mode.
    #[inline]
    pub fn at(&self, path: &[u8]) -> Option<RouteId> {
        let mut out = None;
        self.for_each_candidate(path, |rid| {
            if out.is_none() {
                out = Some(rid);
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_include_all_prefix_nodes() {
        let mut r = Router::new();
        r.insert_prefix(b"/", 1);
        r.insert_prefix(b"/foo", 2);
        r.insert_prefix(b"/foo/bar", 3);

        let mut got = Vec::new();
        r.for_each_candidate(b"/foo/bar/baz", |id| got.push(id));
        assert_eq!(got, vec![1, 2, 3]);
    }
}
