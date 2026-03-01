// somewhere in arc_config build/finalize
let mut router = arc_router::Router::new();
for (i, r) in routes.iter().enumerate() {
    router.insert_prefix(r.path.as_ref(), i as u32);
}
cfg.router = router;
cfg.routes = routes;
