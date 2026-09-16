import { useEffect, useState } from "react";
import {
  Button,
  ButtonGroup,
  Callout,
  Classes,
  Dialog,
  DialogBody,
  DialogFooter,
  Intent,
  Tag,
  TextArea,
} from "@blueprintjs/core";
import { parseInput, type RouteInput } from "../../api";
import { createSample } from "../../sample";

interface NewJobDialogProps {
  isOpen: boolean;
  dark: boolean;
  existingJobIds: ReadonlySet<string>;
  onClose: () => void;
  onCreate: (request: RouteInput) => void;
}

interface ValidationResult {
  valid: boolean;
  message: string;
}

function sampleText() {
  return JSON.stringify(createSample(), null, 2);
}

export function NewJobDialog({
  isOpen,
  dark,
  existingJobIds,
  onClose,
  onCreate,
}: NewJobDialogProps) {
  const [input, setInput] = useState(sampleText);
  const [validation, setValidation] = useState<ValidationResult | null>(null);

  useEffect(() => {
    if (isOpen) {
      setInput(sampleText());
      setValidation(null);
    }
  }, [isOpen]);

  function parseAndValidate() {
    try {
      const request = parseInput(input);
      if (existingJobIds.has(request.job_id)) {
        throw new Error(
          `job_id "${request.job_id}"가 이 세션에 이미 존재합니다.`,
        );
      }
      setValidation({ valid: true, message: "유효함" });
      return request;
    } catch (error) {
      setValidation({ valid: false, message: (error as Error).message });
      return null;
    }
  }

  function format() {
    const request = parseAndValidate();
    if (request) setInput(JSON.stringify(request, null, 2));
  }

  function reset() {
    setInput(sampleText());
    setValidation(null);
  }

  function create() {
    const request = parseAndValidate();
    if (request) onCreate(request);
  }

  return (
    <Dialog
      className="new-job-dialog"
      isOpen={isOpen}
      isCloseButtonShown={false}
      onClose={onClose}
      portalClassName={dark ? Classes.DARK : undefined}
      title="새 Job"
      icon="new-object"
      canEscapeKeyClose
    >
      <DialogBody>
        <div className="dialog-editor-heading">
          <span>최적화 요청 JSON</span>
          <ButtonGroup size="small" variant="minimal">
            <Button icon="code" onClick={format}>
              포맷
            </Button>
            <Button icon="reset" onClick={reset}>
              샘플 복원
            </Button>
          </ButtonGroup>
        </div>
        <TextArea
          aria-label="요청 JSON"
          className="json-editor new-job-editor"
          fill
          intent={validation?.valid === false ? Intent.DANGER : Intent.NONE}
          spellCheck={false}
          autoCapitalize="off"
          value={input}
          onChange={(event) => {
            setInput(event.target.value);
            setValidation(null);
          }}
        />
        <div className="dialog-validation" aria-live="polite">
          {validation?.valid ? (
            <Tag icon="tick" intent={Intent.SUCCESS} minimal>
              유효함
            </Tag>
          ) : validation ? (
            <Callout
              compact
              intent={Intent.DANGER}
              role="alert"
              title="요청 형식 오류"
            >
              {validation.message}
            </Callout>
          ) : null}
        </div>
      </DialogBody>
      <DialogFooter
        actions={
          <>
            <Button onClick={onClose}>취소</Button>
            <Button icon="tick" onClick={parseAndValidate}>
              검증
            </Button>
            <Button icon="play" intent={Intent.PRIMARY} onClick={create}>
              Job 생성
            </Button>
          </>
        }
      />
    </Dialog>
  );
}
