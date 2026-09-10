/// Describes the attributes shown by Python's plain-text and HTML representations.
///
/// Python classes using [`PyDisplay`] should expose the returned names through a
/// `__display_fields__` class attribute. The names describe displayable state,
/// not necessarily constructor arguments, so the protocol also fits result
/// objects such as fills and cancellations.

pub trait DisplayFields {
    fn display_fields() -> Vec<&'static str>;
}

#[cfg(feature = "python")]
use pyo3::prelude::*;

/// Reusable Python representation mixin.
///
/// Subclasses provide a `__display_fields__` class attribute containing the
/// attribute names to render. Both methods resolve that attribute on the actual
/// Python subclass, which allows each class to choose its own displayed state.

#[cfg_attr(
    feature = "python",
    pyclass(name = "Display", module = "lobo.base", subclass)
)]
pub struct PyDisplay;

#[cfg(feature = "python")]
#[pymethods]
impl PyDisplay {
    fn __repr__(slf: PyRef<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let obj = (&slf)
            .into_pyobject(py)
            .expect("PyRef -> Python object is infallible");
        let obj = obj.as_any();
        let class = obj.get_type();

        let class_name: String = class.getattr("__name__")?.extract()?;
        let display_fields: Vec<String> = class.getattr("__display_fields__")?.extract()?;
        let mut fields = Vec::with_capacity(display_fields.len());

        for name in display_fields {
            let value = obj.getattr(name.as_str())?;
            fields.push(format!("{name}={}", value.repr()?));
        }

        Ok(format!("{}({})", class_name, fields.join(", ")))
    }

    fn _repr_html_(slf: PyRef<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let obj = (&slf)
            .into_pyobject(py)
            .expect("PyRef -> Python object is infallible");
        let obj = obj.as_any();
        let class = obj.get_type();

        let class_name: String = class.getattr("__name__")?.extract()?;
        let display_fields: Vec<String> = class.getattr("__display_fields__")?.extract()?;
        let mut rows = String::new();

        for name in display_fields {
            let value = obj.getattr(name.as_str())?.str()?.to_string();

            rows.push_str(&format!(
                concat!(
                    "<tr>",
                    "<th style=\"padding:.25em .75em .25em 0\">{}</th>",
                    "<td>{}</td>",
                    "</tr>"
                ),
                escape_html(&name),
                escape_html(&value),
            ));
        }

        Ok(format!(
            concat!(
                "<table style=\"border-collapse:collapse;text-align:left\">",
                "<caption style=\"font-weight:600;text-align:left;margin-bottom:.4em\">{}</caption>",
                "<tbody>{}</tbody>",
                "</table>"
            ),
            escape_html(&class_name),
            rows,
        ))
    }
}

#[cfg(feature = "python")]
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::DisplayFields;

    struct ExampleDisplay;

    impl DisplayFields for ExampleDisplay {
        fn display_fields() -> Vec<&'static str> {
            vec!["price", "quantity"]
        }
    }

    #[test]
    fn display_fields_are_supplied_by_implementors() {
        assert_eq!(ExampleDisplay::display_fields(), ["price", "quantity"]);
    }

    #[cfg(feature = "python")]
    #[test]
    fn html_escaping_covers_all_special_characters() {
        assert_eq!(super::escape_html("<&>\"'"), "&lt;&amp;&gt;&quot;&#39;");
        assert_eq!(super::escape_html("plain"), "plain");
    }
}
