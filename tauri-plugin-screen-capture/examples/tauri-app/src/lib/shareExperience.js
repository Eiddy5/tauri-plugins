export function shareExperienceMode({ pickerOpen, session }) {
  if (session) return "sharing"
  return pickerOpen ? "picker" : "landing"
}
