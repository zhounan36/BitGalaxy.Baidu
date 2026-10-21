import { useEffect, useState } from "react";

export default function Modal({ modal, onClose }) {
  const [val, setVal] = useState(modal.initial || "");
  useEffect(() => setVal(modal.initial || ""), [modal]);

  const submit = () => {
    const v = val.trim();
    if (modal.type === "prompt" && !v) return;
    onClose();
    modal.onOk(v);
  };

  return (
    <div className="modal-mask" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h3>{modal.title}</h3>
        {modal.type === "confirm" ? (
          <p className="modal-text">{modal.text}</p>
        ) : (
          <input
            autoFocus
            value={val}
            onChange={(e) => setVal(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && submit()}
          />
        )}
        <div className="modal-ops">
          <button className="btn ghost" onClick={onClose}>
            取消
          </button>
          <button
            className={"btn " + (modal.danger ? "danger" : "primary")}
            onClick={submit}
          >
            确定
          </button>
        </div>
      </div>
    </div>
  );
}
