use anyhow::Context;
use minijinja::{Environment, Value};

pub fn build_env() -> Environment<'static> {
    let mut env = Environment::new();
    env.add_template_owned(
        "layout.html",
        include_str!("../../templates/admin/layout.html").to_string(),
    )
    .expect("layout.html template is invalid");
    env.add_template_owned(
        "repo_list.html",
        include_str!("../../templates/admin/repo_list.html").to_string(),
    )
    .expect("repo_list.html template is invalid");
    env.add_template_owned(
        "script_edit.html",
        include_str!("../../templates/admin/script_edit.html").to_string(),
    )
    .expect("script_edit.html template is invalid");
    env
}

pub fn render(env: &Environment<'static>, template: &str, ctx: Value) -> anyhow::Result<String> {
    let tmpl = env
        .get_template(template)
        .with_context(|| format!("template {template} not found"))?;
    tmpl.render(ctx)
        .with_context(|| format!("failed to render {template}"))
}
