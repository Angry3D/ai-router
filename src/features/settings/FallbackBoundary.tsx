import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripHorizontal } from "lucide-react";

import { useLanguage } from "../language/useLanguage";
import { FALLBACK_BOUNDARY_ID } from "./routeOrderSequence";

interface FallbackBoundaryProps {
  participantCount: number;
  routeCount: number;
  disabled: boolean;
  pending: boolean;
}

interface FallbackBoundaryVisualProps extends FallbackBoundaryProps {
  dragging?: boolean;
  overlay?: boolean;
}

function FallbackBoundaryContent({ label }: { label: string }) {
  return (
    <>
      <span className="fallback-boundary-label">{label}</span>
      <span className="fallback-boundary-rail" aria-hidden="true">
        <GripHorizontal className="fallback-boundary-grip" size={14} />
      </span>
    </>
  );
}

export function FallbackBoundaryVisual({
  participantCount,
  routeCount,
  disabled,
  pending,
  dragging = false,
  overlay = false,
}: FallbackBoundaryVisualProps) {
  const { messages } = useLanguage();
  return (
    <div
      className={`fallback-boundary${dragging ? " is-dragging" : ""}${pending ? " is-pending" : ""}${overlay ? " is-drag-overlay" : ""}`}
      aria-hidden="true"
      data-fallback-boundary-overlay={overlay ? "" : undefined}
    >
      <FallbackBoundaryContent label={messages.routes.boundary.label} />
      <span className="visually-hidden">
        {participantCount} / {routeCount}
        {disabled ? " disabled" : ""}
      </span>
    </div>
  );
}

export function FallbackBoundary({
  participantCount,
  routeCount,
  disabled,
  pending,
}: FallbackBoundaryProps) {
  const { messages } = useLanguage();
  const boundary = messages.routes.boundary;
  const {
    attributes,
    listeners,
    setActivatorNodeRef,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({
    id: FALLBACK_BOUNDARY_ID,
    disabled,
  });

  return (
    <div
      ref={setNodeRef}
      className={`fallback-boundary-slot${isDragging ? " is-source-placeholder" : ""}`}
      data-sortable-item={FALLBACK_BOUNDARY_ID}
      style={{
        transform:
          !isDragging && transform
            ? CSS.Transform.toString({ ...transform, x: 0 })
            : undefined,
        transition,
      }}
    >
      <button
        ref={setActivatorNodeRef}
        type="button"
        className={`fallback-boundary${isDragging ? " is-dragging" : ""}${pending ? " is-pending" : ""}`}
        disabled={disabled}
        aria-busy={pending}
        title={boundary.dragHandleTitle}
        data-sortable-handle={FALLBACK_BOUNDARY_ID}
        {...attributes}
        {...listeners}
        aria-label={boundary.dragHandleLabel}
        aria-describedby="fallback-boundary-description"
      >
        <FallbackBoundaryContent label={boundary.label} />
      </button>
      <span id="fallback-boundary-description" className="visually-hidden">
        {boundary.description(participantCount, routeCount)}
        {pending ? boundary.savingSuffix : ""}
      </span>
    </div>
  );
}
