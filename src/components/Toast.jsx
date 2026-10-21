export default function Toast({ toast }) {
  return <div className={"toast " + toast.type}>{toast.message}</div>;
}
