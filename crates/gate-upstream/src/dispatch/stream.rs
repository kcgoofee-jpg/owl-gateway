//! 已打开的图片流：逐块读、按已确认张数结算，被丢弃时在后台补结算。

use super::*;

/// 已打开的上游图片流。调用方逐块读取，确认完整的最终图片后累加 `completed_images`，
/// 最后调用 [`ImageStream::finish`]；被丢弃时按已确认张数在后台结算。
pub struct ImageStream<H: Hooks> {
    pub(super) response: Option<wreq::Response>,
    pub(super) flight: Option<InFlight<H>>,
    pub(super) token: Arc<Token>,
    pub completed_images: usize,
}

impl<H: Hooks> ImageStream<H> {
    pub fn headers(&self) -> Option<&HeaderMap> {
        self.response.as_ref().map(|r| r.headers())
    }

    /// 取走正文字节流（只能取一次）。已按 Content-Encoding 解压，边收边给，不整包缓冲。
    pub fn take_body(
        &mut self,
    ) -> Option<impl futures::Stream<Item = Result<Bytes, wreq::Error>> + use<H>> {
        self.response.take().map(|r| r.bytes_stream())
    }

    /// 结算：有确认的图就记成功并标记账号健康。
    pub async fn finish(mut self) {
        self.response = None;
        if let Some(mut fl) = self.flight.take() {
            let count = self.completed_images;
            fl.succeeded = count > 0;
            fl.image_count = count;
            let inner = fl.inner.clone();
            fl.finish().await;
            if count > 0 {
                self.token.mark_ok(&inner.hooks);
            }
        }
    }
}

impl<H: Hooks> Drop for ImageStream<H> {
    fn drop(&mut self) {
        if let Some(mut fl) = self.flight.take() {
            let count = self.completed_images;
            fl.succeeded = count > 0;
            fl.image_count = count;
            if count > 0 {
                self.token.mark_ok(&fl.inner.hooks);
            }
            drop(fl); // InFlight 的 Drop 在后台完成结算
        }
    }
}
