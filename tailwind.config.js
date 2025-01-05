/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ['./templates/*.html'],
  theme: {
    extend: {
      colors: {
        'navy': '#14213d',
        'body-grey': '#f5f5f5',
        'div-grey': '#e5e5e5',
        'yellow': '#ffc935'
      },
      fontFamily: {
        display: 'Oswald, ui-serif',
        content: 'Quattrocento, ui-serif'
      }
    },
  },
}
