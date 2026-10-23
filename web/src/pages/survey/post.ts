/** 成员问卷页提交用：只靠 Discord 会话 Cookie，不带 Key。出错抛 ApiError，message 是给成员看的原因。 */
import { ApiError, sendJSON } from "../../lib/api";

export async function postMember(url: string, body: unknown): Promise<void> {
  try {
    await sendJSON("POST", url, body);
  } catch (x) {
    if (!(x instanceof ApiError)) throw new ApiError(0, "网络错误，稍后再试");
    if (x.status === 401) throw new ApiError(401, "登录状态已过期，请刷新页面后重新登录再提交。");
    throw x;
  }
}
