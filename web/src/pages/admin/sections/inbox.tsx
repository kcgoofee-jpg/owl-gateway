/** 来信：成员反馈与问卷。 */
import { TabPage } from "./_tabs";
import { Feedback } from "./inbox/Feedback";
import { Survey } from "./inbox/Survey";

export default function Inbox() {
  return (
    <TabPage
      title="来信"
      base="/admin/inbox"
      label="来信类型"
      tabs={[
        { id: "feedback", label: "反馈", view: Feedback },
        { id: "survey", label: "问卷", view: Survey },
      ]}
    />
  );
}
