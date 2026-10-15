use desigma_gpu::{GpuContext, GpuError, Renderer};

#[test]
fn probe_adapter() {
    match GpuContext::new() {
        Ok(c) => eprintln!("adapter: {:?}", c.adapter_info),
        Err(e) => eprintln!("SKIP: {e}"),
    }
}

#[test]
fn renderer_pipelines_initialize_on_available_adapter() {
    let ctx = match GpuContext::new() {
        Ok(ctx) => ctx,
        Err(GpuError::NoAdapter(e)) => {
            eprintln!("SKIP: no GPU adapter ({e})");
            return;
        }
        Err(e) => panic!("GPU device initialization failed: {e}"),
    };
    eprintln!("adapter: {:?}", ctx.adapter_info);
    Renderer::new(&ctx.device, &ctx.queue).expect("renderer pipelines must validate on an available adapter");
}
