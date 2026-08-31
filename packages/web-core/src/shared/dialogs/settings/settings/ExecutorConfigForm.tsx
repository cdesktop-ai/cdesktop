import { useMemo, useEffect, useState, useCallback } from 'react';
import Form from '@rjsf/core';
import type { IChangeEvent } from '@rjsf/core';
import { RJSFValidationError } from '@rjsf/utils';
import validator from '@rjsf/validator-ajv8';
import { useTranslation } from 'react-i18next';
import { BaseCodingAgent } from 'shared/types';
import { settingsRjsfTheme } from './rjsf/theme';
import { SettingsSaveBar } from './SettingsComponents';

interface ExecutorConfigFormProps {
  executor: BaseCodingAgent;
  value: unknown;
  onChange?: (formData: unknown) => void;
  onSave?: (formData: unknown) => Promise<void>;
  onDiscard?: () => void;
  disabled?: boolean;
  saving?: boolean;
  isDirty?: boolean;
}

import schemas from 'virtual:executor-schemas';

// Fields hidden from the rendered schema per executor. Saved values are
// preserved (we filter at render time, not in the data). To re-show a field,
// remove its name from the array.
const HIDDEN_FIELDS_BY_EXECUTOR: Partial<Record<BaseCodingAgent, string[]>> = {
  [BaseCodingAgent.CLAUDE_CODE]: [
    'claude_code_router',
    'plan',
    'approvals',
    'permission_mode_override',
    'model',
    'effort',
    'dangerously_skip_permissions',
    'disable_api_key',
  ],
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function ExecutorConfigForm({
  executor,
  value,
  onChange,
  onSave,
  onDiscard,
  disabled = false,
  saving = false,
  isDirty = false,
}: ExecutorConfigFormProps) {
  const { t, i18n } = useTranslation('settings');
  const [formData, setFormData] = useState<unknown>(value || {});
  const [validationErrors, setValidationErrors] = useState<
    RJSFValidationError[]
  >([]);

  const schema = useMemo(() => {
    const base = schemas[executor];
    if (!base) return base;
    const hidden = HIDDEN_FIELDS_BY_EXECUTOR[executor];
    if (!hidden || hidden.length === 0) return base;
    const next = { ...base };
    if (isRecord(next.properties)) {
      const properties = { ...next.properties };
      for (const key of hidden) {
        delete properties[key];
      }
      next.properties = properties;
    }
    return next;
  }, [executor]);

  // Custom handler for env field updates
  const handleEnvChange = useCallback(
    (envData: Record<string, string> | undefined) => {
      const newFormData = {
        ...(formData as Record<string, unknown>),
        env: envData,
      };
      setFormData(newFormData);
      if (onChange) {
        onChange(newFormData);
      }
    },
    [formData, onChange]
  );

  const uiSchema = useMemo(() => {
    const ui: Record<string, Record<string, unknown>> = {
      env: { 'ui:field': 'KeyValueField' },
    };
    const props = isRecord(schema?.properties) ? schema.properties : undefined;
    if (props) {
      for (const fieldName of Object.keys(props)) {
        const titleKey = `settings.agents.fields.${fieldName}.title`;
        const descKey = `settings.agents.fields.${fieldName}.description`;
        const fieldUi = (ui[fieldName] = ui[fieldName] || {});
        if (i18n.exists(titleKey, { ns: 'settings' })) {
          fieldUi['ui:title'] = t(titleKey);
        }
        if (i18n.exists(descKey, { ns: 'settings' })) {
          fieldUi['ui:description'] = t(descKey);
        }
      }
    }
    return ui;
  }, [schema, t, i18n]);

  // Pass the env update handler via formContext
  const formContext = useMemo(
    () => ({
      onEnvChange: handleEnvChange,
    }),
    [handleEnvChange]
  );

  useEffect(() => {
    setFormData(value || {});
    setValidationErrors([]);
  }, [value, executor]);

  const handleChange = (event: IChangeEvent<unknown>) => {
    const newFormData = event.formData;
    setFormData(newFormData);
    if (onChange) {
      onChange(newFormData);
    }
  };

  const handleSave = async () => {
    if (onSave) {
      await onSave(formData);
    }
  };

  const handleError = (errors: RJSFValidationError[]) => {
    setValidationErrors(errors);
  };

  if (!schema) {
    return (
      <div className="bg-error/10 border border-error/50 rounded-sm p-4 text-error">
        {t('settings.agents.errors.schemaNotFound', { executor })}
      </div>
    );
  }

  const hasValidationErrors = validationErrors.length > 0;

  return (
    <div className="space-y-4">
      <Form
        schema={schema}
        uiSchema={uiSchema}
        formData={formData}
        formContext={formContext}
        onChange={handleChange}
        onError={handleError}
        validator={validator}
        disabled={disabled}
        liveValidate
        showErrorList={false}
        widgets={settingsRjsfTheme.widgets}
        templates={settingsRjsfTheme.templates}
        fields={settingsRjsfTheme.fields}
      >
        {/* No submit button - SettingsSaveBar handles saving */}
        <></>
      </Form>

      {hasValidationErrors && (
        <div className="bg-error/10 border border-error/50 rounded-sm p-4 text-error">
          <ul className="list-disc list-inside space-y-1">
            {validationErrors.map((error, index) => (
              <li key={index}>
                {error.property}: {error.message}
              </li>
            ))}
          </ul>
        </div>
      )}

      {onSave && (
        <SettingsSaveBar
          show={isDirty}
          saving={saving}
          saveDisabled={hasValidationErrors}
          unsavedMessage={t('settings.agents.save.unsavedChanges')}
          onSave={handleSave}
          onDiscard={onDiscard}
        />
      )}
    </div>
  );
}
