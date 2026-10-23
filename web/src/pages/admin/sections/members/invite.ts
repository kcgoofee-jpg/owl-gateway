/** 发给被邀请人的整段消息。网页只支持 Discord 登录：绑定靠 Discord 里的 /register，不再有「粘贴 Key 登录网页」。 */
export function inviteMessage(site: string, token: string): string {
  return `你好！这是猫头鹰公益站的邀请 Key，只能绑定一个 Discord 账号，请勿转发：

${token}

绑定步骤：
1. 在 Discord 猫头鹰服务器的任意频道输入 /register
2. 在弹出的 key 一栏粘贴上面这串 Key，回车发送
3. 看到「✅ 邀请 Key 已绑定到你的 Discord 账号」就完成了

绑定后打开 ${site} ，点「用 Discord 登录」就能查看额度和接入方法（网页只支持 Discord 登录，不用粘贴 Key）；也可以在 Discord 用 /quota 查看。`;
}
